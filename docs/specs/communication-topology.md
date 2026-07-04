# Spec: Communication Topology Control Surface

- **Status**: Approved design (brainstormed 2026-07-02)
- **Issue**: [#610](https://github.com/wardian-app/Wardian/issues/610)
- **Related**: [#609](https://github.com/wardian-app/Wardian/issues/609) (per-agent privacy flag — out of scope here)

## Strategic Rationale

The graph view today is a read-only projection of three derived signals (same team, shared workspace, same worktree). This spec reforms it into the **control surface for cross-agent communication topology**: an editable, arbitrary, undirected graph — org-chart-like in purpose, free-form in shape — that determines which agents see each other by default.

The topology also changes CLI behavior: an agent running `wardian agent list` sees only its **neighbors** by default, and broadcast/class targets resolve within that neighbor set. This shapes agent attention without restricting capability.

### Core semantic: soft boundary

The topology shapes **default visibility and target resolution, never permissions**. An explicit `send` to an exact name or UUID outside the neighbor set always works. Rationale: a hard boundary would require enforcement in the control API plus escape hatches for workflows and the user; and any deliberate global reader (e.g. the evolver) bypasses visibility filters anyway. Consequently the topology provides **no privacy guarantees** — privacy is an agent-level logging control, tracked separately in #609.

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
  ],
  "suppressed_seed_pairs": [
    { "a": "<agent-uuid>", "b": "<agent-uuid>" }
  ]
}
```

- Edges are **undirected and canonical**: `a < b` lexicographically; deduped on save (same canonicalization as `graphProjection.ts` edge keys).
- Written atomically (temp file + rename), like the watchlist state file.
- Only **manual** edges are stored. Workspace fallback is resolved at read time, but team membership seeds ordinary manual edges instead of producing read-time rule edges.
- Edges reference agent UUIDs. Edges naming deleted agents are ignored at read time and garbage-collected on the next save.
- `ignored_pairs` holds ghost-edge dismissals (see UI section) so they are durable and inspectable on disk. (rev. 2026-07-03: with team edges seeded as manual, ignores no longer apply to anything but ghosts.)
- `suppressed_seed_pairs` holds team-seed tombstones. Removing a team-backed edge while both agents remain on a team records the pair here so later seed passes do not recreate it; manually adding the edge clears the tombstone.
- A missing or corrupt file resolves to an empty topology — it must never block agent listing.

### Neighbor-set resolution (formerly "community", in `wardian-core`)

New module `crates/wardian-core/src/topology.rs`:

1. `load_topology(home) -> Topology` / `save_topology(home, &Topology)` — parse, validate, canonicalize.
2. `resolve_neighbors(agent_uuid, &Topology, &[TeamSummary], &[AgentRow]) -> NeighborsView`
3. Mutation helpers `add_edge` / `remove_edge` / `ignore_pair` / `unignore_pair` used by Tauri commands.

**Resolver definition (rev. 2026-07-03 — team seeding replaces the team-clique rule):**

```text
neighbors(agent) = manual neighbors, with workspace-fallback when the agent has none
```

Teams are no longer a read-time rule. Instead, **teams seed ordinary manual edges**: when a team is created or a member is added, clique edges among its members are written to `topology.json` as plain manual edges (idempotent; existing edges untouched). After seeding, the graph is user-owned — deleting a team-born edge is a real delete and later seed passes skip the pair via `suppressed_seed_pairs`. Removing an agent from a team leaves its edges in place; connections persist until explicitly deleted.

One built-in rule remains:

| Rule id | Semantics |
|---|---|
| `workspace-fallback` | Only when the agent has **no manual edges**: its workspace-mates |

Each neighbor is tagged with *why* it is visible: `manual` or `rule:workspace-fallback`. The CLI shows reasons in verbose output.

**Migration:** `topology.json` gained a `version: 2` marker for the one-time team seed, then `version: 3` for durable team-seed tombstones. On app startup, a pre-version-2 (or missing) file triggers a one-time seed of cliques for all current teams, then saves at the current schema version. Existing version-2 files are not reseeded merely because schema version 3 exists; instead, current team pairs that are missing from the v2 edge list are recorded in `suppressed_seed_pairs` before the file is saved as v3. This preserves any team-born edges the user already deleted. The CLI never migrates (read-only consumer); until the app has run once, team-born edges simply don't exist yet.

**Opt-in moment:** drawing an agent's first manual edge (including team-seeded ones) disengages `workspace-fallback` for that agent — its neighbors become exactly what the graph says. Per-agent, no global mode flag.

**Rejected alternative (superseded 2026-07-02 design):** rule-derived team-clique edges with `ignored_pairs` overrides. In practice the dual edge-origin model produced repeated divergence bugs (resolver vs snapshot filtering, dim rule textures, override UI that didn't visibly work) — the read-time-derivation benefit ("nothing to sync") did not pay for the complexity.

## CLI Behavior

### `wardian agent list`

- `--scope neighbors` — **new default when `WARDIAN_SESSION_ID` is set**: self + resolved neighbors.
- `--scope workspace` / `--scope all` — unchanged; `all` is the standing escape hatch.
- **No session identity** (human in a plain shell): default remains today's `workspace` behavior. Nothing changes for human use.
- `--verbose` adds the visibility reason per row.

### `wardian send` / `ask` target resolution (agent callers only)

- `--to all` → broadcasts to the sender's neighbor set, not the global roster.
- `--to class:X` → resolves within the neighbors.
- Bare names → neighbors-first; if no neighbor matches, fall back to global exact match (soft-boundary contract: never blocks explicit targeting).
- UUIDs → always global, always exact.
- New `--scope all` flag on `send` restores global broadcast/class resolution for orchestrators.

### Unchanged

`team list`, `watchlist list`, and `conversation list` remain global. Conversations stay global deliberately — the evolver depends on reading all of them; privacy is #609's job.

### Implementation path

The CLI takes no control-API dependency: `main.rs` loads `topology.json`, watchlist state, and the agent DB directly (all read paths exist today) and calls the same `resolve_neighbors` as the app. Ghost edges are a UI-only concept; the CLI neighbor set is manual ∪ rules only.

### Guidance surfaces (ships in the same PR as the CLI default change)

Because enforcement is soft, agent-facing documentation is half the feature:

- Update the bundled `wardian-cli` skill: "your default listing is your neighbors; `--scope all` exists but reach for it only when your task genuinely spans multiple neighbor sets."
- Sweep `~/.wardian/common/AGENTS.md` and per-class `AGENTS.md` sources for hardcoded `--scope all` examples.

## Backend Surface (Tauri)

New commands, thin wrappers over `wardian-core::topology`:

- `get_topology()` → manual edges + ignored pairs. (rev. 2026-07-03: no rule-derived edges in the snapshot; teams seed manual edges instead.)
- `add_topology_edge(a, b)` / `remove_topology_edge(a, b)` — canonicalize, save atomically, emit `topology_changed`. Removing a pair that is still produced by team membership writes a `suppressed_seed_pairs` tombstone; adding the pair manually clears that tombstone.
- `ignore_pair(a, b)` / `unignore_pair(a, b)` — ghost dismissal. `resolve_neighbors` skips workspace-fallback reasons for ignored pairs; manual edges are unaffected (creating one implies intent).
- `get_pair_activity()` → `[{ pair, last_message_at, active_ask, direction }]`, derived from the conversation store; updated incrementally over the existing conversation event stream (no polling). `active_ask` is **time-bounded**: an unresolved ask older than the recency window (1 hour) no longer counts as ongoing — stale asks must not animate forever.

State sovereignty: the frontend never computes or caches topology truth. It renders what the resolver returns over IPC; all mutations round-trip through the backend. The UI remains a passive observer/editor.

## Graph View UI

### Editing

- **Create edge**: linking is always available — no modal "connect mode". Shift-drag from node A to node B (plain drag still repositions), or use the inspector's "Add connection…" picker → `add_topology_edge` → save + `topology_changed` → re-project. Optimistic update is safe since projection is pure. A persistent hint in the graph toolbar states the gesture.
- **Delete edge**: select edge, Delete key or inspector × → `remove_topology_edge`. (rev. 2026-07-03: all real edges are manual, so delete is uniform; the override/restore UI is removed.)
- **Inspector**: the Relationships panel mirrors the neighbors textually — each member with its reason tag, per-row disconnect for manual edges, and an "Add connection…" searchable picker.

### Two-channel edge encoding

No channel carries two meanings. Validated via interactive mockups during the design session.

| Channel | Encodes | Values |
|---|---|---|
| **Texture** (static) | Origin | solid = manual · sparse dash = unmapped traffic (ghost) — (rev. 2026-07-03: the dotted rule texture is gone with the team-clique rule) |
| **Color + motion** (dynamic) | Communication state | ongoing = cyan + directed particles · recent = light cyan fading with age (1-hour default window) · dormant = clearly visible neutral gray |

- **Structure must stay legible** (rev. 2026-07-03): dormant manual edges are topology structure, not activity residue — they render at full readability (no heavy alpha fade). Only the *activity* brightening varies with recency.
- **Layout follows the topology** (rev. 2026-07-03): node positions come from a force-directed layout over the communication edges (seeded deterministically, re-run on `topology_changed`), so drawing a manual edge visibly pulls agents together. Team membership no longer dictates position; edgeless agents settle on the periphery.

- Nodes keep the existing status color system (theme variables) and selection ring. The recent-activity **halo node is removed** — recency lives on edges now, and the phantom halo read as a larger node and degraded hit-testing. Node labels must resolve color/size from theme variables for readability in both themes.
- Particles flow in message direction; during a pending ask, the stream drifts toward the agent that owes the reply — direction *is* the pending-ask indicator.
- **Idle cost is zero**: the animation loop runs only while at least one edge is genuinely ongoing (time-bounded `active_ask`), as a single rAF chain; camera moves redraw within it, never spawn parallel loops. With nothing ongoing, the overlay renders statically on data/camera changes only.
- The three legacy lenses (`same_team`, `shared_workspace`, `same_worktree`) remain as read-only overlay lenses, off by default; the communication topology is the always-on base layer, separated in the lens toolbar.
- Cut from v1: edge thickness scaled by message volume (thickness would compete with brightness for the "activity" reading).

### Ghost edges (unmapped traffic)

- **Detection**: any pair in `pair_activity` with recent traffic but no manual edge and not covered by workspace-fallback. Computed in the resolver layer.
- **Rendering**: sparse-dash texture; state colors follow the same palette as real edges.
- **Lifecycle**: exists only while traffic is within the recency window; fades out with it.
- **Interaction**: click → inspector shows "Unmapped communication: A ↔ B, last message Nm ago" with **Formalize** (write a manual edge) or **Ignore** (append to `ignored_pairs`; new traffic does not resurrect the ghost unless cleared).

## Edge Cases

- Missing/corrupt `topology.json` → empty topology; listing never fails.
- Edge referencing a deleted agent → ignored at read, GC'd on next save.
- Duplicate / self-edges → rejected at canonicalization.
- Agent in multiple teams → union of seeded cliques (idempotent; overlapping pairs deduped by canonicalization).
- Concurrent writers (app + hypothetical CLI mutation) → atomic rename, last-writer-wins; acceptable for a user-edited artifact at this write rate.
- Fresh install / no topology file → every agent is edgeless and (typically) teamless, so `workspace-fallback` reproduces today's behavior exactly. **No migration needed.**

## Testing

Per the E2E layer boundary rules, lowest layer that proves each behavior:

- **Rust unit** (`wardian-core`): resolver table-tests — manual∪rule union, canonical dedup, workspace-fallback engagement/disengagement on first edge or team join, deleted-agent GC, ignored pairs, corrupt/missing file → empty topology.
- **TS unit**: `graphProjection` origin/state channel assignment, ghost derivation, legacy lenses as overlays.
- **Browser E2E**: seeded topology fixture renders edges; drag-connect creates an edge; delete removes it; ghost appears from seeded conversation activity (mock provider `multi_turn`).
- **Native E2E**: app writes an edge → `wardian agent list` under `WARDIAN_SESSION_ID` returns exactly the neighbors; broadcast resolution scoped; `--scope all` unaffected.

## Non-Goals

- **Privacy.** A soft visibility boundary cannot provide it; per-agent conversation-logging control is #609.
- **Hard enforcement / ACLs.** Explicit exact-name/UUID communication always works.
- **Directed or typed edges.** Undirected peer edges only in v1; hierarchy can layer on later.
- **User-defined rules.** The rule abstraction anticipates them, but v1 ships only `workspace-fallback` (rev. 2026-07-03: team-clique was replaced by manual-edge seeding).
- **Volume-scaled edge thickness.** Cut from v1 (channel conflict with brightness).

## Decision Log

| Decision | Choice | Alternatives considered |
|---|---|---|
| Enforcement | Soft boundary (visibility only) | Hard ACL; hard with per-edge overrides |
| Edge model | Undirected peers | Directed with roles; undirected with typed labels |
| Neighbor-set definition (formerly "community") | Direct neighbors only | Connected component; neighbors ∪ workspace |
| Edge origin | Manual + rule-derived (team clique; workspace fallback) | Fully manual; all derived signals as rules with tombstones |
| Source of truth | `topology.json` in `WARDIAN_HOME` + shared resolver in `wardian-core` | SQLite table; app-as-truth via control API |
| Team edge visuals | Generic "rule-derived" texture, rule named in inspector | Team-specific styling (rejected: blocks future rule-based relations) |
| Ghost attention cue | Texture + inspector badge, standard state palette | Amber color (rejected: violated two-channel encoding) |
| Team edges (rev. 2026-07-02) | Connected by default, deletable via `ignored_pairs` override | Managed/undeletable with deep-link to team editor (rejected after use: users expect direct edge control) |
| Team edges (rev. 2026-07-03, supersedes above) | Teams **seed** plain manual edges on membership-change events and first migration; delete is a real delete recorded in `suppressed_seed_pairs`; edges persist on team leave | Rule-derived with overrides (rejected after use: resolver/snapshot divergence bugs, dim dual-texture rendering, override UI without visible effect); no team edges at all (rejected: teams would lose topological meaning) |
| Graph layout (rev. 2026-07-03) | Force-directed over communication edges, re-run on topology change | Team-cluster layout (rejected: manual edges had no visible effect on arrangement) |
| Dormant edge rendering (rev. 2026-07-03) | Full-legibility neutral color; only activity varies with recency | Heavy alpha fade (rejected: structure was nearly invisible) |
| Activity animation (rev. 2026-07-02) | Time-bounded `active_ask`; single rAF only while ongoing | Unbounded asks + always-on loop (rejected: stale asks animated forever, parallel rAF chains lagged the view) |
| Recent-node halo (rev. 2026-07-02) | Removed (recency encoded on edges) | Keep phantom halo node (rejected: read as larger node, clunky hit-testing) |
| Edge creation (rev. 2026-07-02) | Always-on Shift-drag + inspector picker, no mode toggle | Modal connect mode (rejected: unclear state) |
