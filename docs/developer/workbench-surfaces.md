# Workbench Surfaces

Wardian's central workspace is a restorable workbench of pane-local surface
tabs. The workbench document describes layout and presentation state; existing
domain stores and the Rust backend remain authoritative for agents, workflows,
Queue entries, library assets, and live runtimes.

## Ownership Boundaries

The central path is deliberately layered:

```text
NavigationService -> WorkbenchStore -> WorkbenchDocumentV1
                                      |
SurfaceRegistry ----------------------+
                                      |
DockviewLayoutAdapter <---------------+
```

- `WorkbenchDocumentV1` owns the normalized split tree, groups, tab placement,
  active IDs, recently closed surfaces, and shell dimensions.
- `SurfaceRegistry` owns type-specific open, render, runtime, close, state,
  command, badge, and restore contracts.
- `NavigationService` is the mutation boundary for opening, focusing, closing,
  closing groups, and resetting. Structural commands such as split, move,
  join, ratio, and active-tab changes use the store's validated command API.
  Destructive navigation runs close guards before committing changes.
- `DockviewLayoutAdapter` projects Wardian's document through Dockview's public
  API. Dockview is not a source of truth and its JSON is never persisted.
- Surface instances own bounded presentation state only. They must reference
  domain resources by stable keys instead of copying domain or runtime truth.

The left icon rail remains an auxiliary-tool selector. The right roster owns
monitoring and command-target selection. Actions in either region may call the
navigation service, but neither region owns the central workbench model.

### Pane chrome and commands

Persistent pane chrome is intentionally small: pane-local tabs, `+` to append
an inline New Tab launcher, and `...` for pane actions. Tab actions belong in tab context menus;
split, move, join, zoom, close-group, and other structural actions belong in
the relevant pane/tab menus. Globally registered structural commands, including
split, move, and zoom, also appear in the actual Ctrl/Cmd+Shift+P command
palette.

Do not add a permanent workbench command bar or restore global titlebar view or
command buttons. Menu, keyboard, and palette entry points share the canonical
Wardian command model rather than mutating Dockview directly; drag/drop always
has a menu or keyboard equivalent.

## Stable Identities

Use each identity for one purpose:

- `surface_id` identifies one persisted presentation instance.
- `surface_type` selects its registry definition.
- `resource_key` optionally identifies a stable domain resource, such as an
  agent session UUID.
- `group_id` identifies the tab group containing the surface.
- live runtime IDs belong to the backend and never replace `surface_id` in the
  persisted document.

An empty group is valid. It derives the Home UI without persisting a synthetic
tab. A user-created `new-tab` surface is instead a canonical allow-multiple
placeholder: navigation replaces it in place when the user chooses a surface,
or discards it without close history when an existing singleton is focused.
The model rejects placeholder-only discard for every other surface type.

## Registry Contract

`src/features/workbench/surfaceRegistry.ts` implements the registry contract.
Core definitions and their explicit order are assembled in
`src/features/workbench/coreSurfaceRegistry.ts`; core-view state metadata lives
under `surfaces/coreSurfaceMetadata.ts`, and their React renderers live in
`surfaces/coreSurfaceDefinitions.tsx`. Registered definitions are copied into
immutable records. A definition supplies:

- a stable `type`, title, icon, commands, and optional badges;
- `open_policy`, `render_policy`, `runtime_policy`, and `close_policy`;
- `state_schema_version`, `max_state_bytes`, a default state, and strict
  serialize/restore functions;
- optional resource-key resolution, existing-instance resolution, and an
  asynchronous close guard.

Registration rejects duplicate or reserved types, invalid policies, unsafe
state versions, and state limits outside `1..65536` bytes. Serialized state must
be canonical JSON; functions, symbols, non-finite numbers, cycles, and other
non-JSON values fail validation.

### Open policies

| Policy | Normal open | Explicit duplicate / Open to Side |
|---|---|---|
| `singleton` | Focus the existing type instance | Focus the existing type instance |
| `focus_resource` | Focus the instance matching `resource_key` | Create another presentation of the same resource |
| `allow_multiple` | Create a new instance | Create a new instance |

Normal Agent Session opens use `focus_resource`. The agent runtime is shared;
duplicate surfaces are independent presentations of it.

Files also uses `focus_resource`. Its canonical resource key is either
`file:<slash-normalized-canonical-path>` or, in the future artifact slice,
`artifact:<artifact-id>`. These identities never collapse into each other even
when an artifact is backed by the same file.

An Explorer or restored path is provisional until `open_file_resource` returns
its backend-owned `resource_id`. The visible Files surface then applies
`canonicalize_resource` as one Workbench transaction. A normal alias open
focuses and removes itself in favor of an existing canonical presentation,
even when that presentation is in another pane. Only `open_to_side` carries
ephemeral explicit-duplicate provenance through the rekey of both it and its
matching source presentation; the pair converges to the same canonical key and
remains in separate panes regardless of which response publishes first.
Repeated same-key acknowledgements do not consume this provenance while the
matching presentation still has its provisional key.
The frontend never resolves symlinks, junctions, or filesystem authority.
Canonicalization re-resolves the current Workbench document after a stale
compare-and-swap. A close guard cancellation remains a user veto, while a
concurrent transaction is retried with fresh aliases so restored tabs converge
without retaining a permanently provisional identity.

### Render policies

- `keep_alive` retains the logical component while its tab is hidden. Expensive
  children still have to respect shared renderer budgets.
- `suspend_when_hidden` retains registered presentation state but releases
  heavy children after the bounded hidden grace period. Graph and Garden use a
  30-second production grace.
- `recreate_from_state` lets hidden content unmount and rebuild from registered
  state when it becomes visible.

The adapter maps `recreate_from_state` to Dockview's visible-only renderer.
Surface components implement any finer-grained suspension required by their
policy; mount behavior alone must not become a lifecycle contract.

### Runtime and close policies

`view_only` means that closing the surface affects presentation state only.
`runtime_backed` means the surface displays a separately owned runtime; it does
not give the surface authority to stop that runtime.

`close_view` can close immediately. `confirm_if_dirty` uses `can_close` to
return `allow` or `cancel`; Library and Workflows use dirty-state guards. Group
close and workbench reset evaluate every affected guard in deterministic visual
order, await required saves, and commit once. A cancel or failed save leaves the
document and durable revision unchanged.

Closing an Agent Session or every presentation of an agent never pauses,
kills, clears, or removes the agent. Runtime lifecycle actions remain explicit
backend commands.

## State Limits and Versioning

The frontend and Rust validators enforce the same V1 limits:

| Boundary | Limit |
|---|---:|
| Entire UTF-8 JSON document | 2 MiB |
| One surface state | 64 KiB, or a smaller registry limit |
| Split-tree depth | 64 nodes |
| Recently closed surfaces | 20 |
| Split ratio | `0.1..0.9` |

Every open surface appears exactly once in one group, every group appears
exactly once in the acyclic split tree, and active IDs must refer to members of
their collections. The pure command model validates both the input document and
the complete candidate document. Rejected commands return the original object.

Each definition owns its versioned restore contract through
`state_schema_version` and `restore_state`; future state migrations must be
sequential and tested there. A restore failure yields a recoverable placeholder
instead of silently discarding the tab. Unknown surface types are retained as
inert opaque JSON within the same size limit; Wardian does not execute or merge
that state until a matching definition is installed and validates it.

### Files foundation state and runtime

`FilesSurfaceStateV1` serializes only `resource_kind`, `mode`,
`transient_preview`, `review_drawer_open`, `selected_version_id`, and
`optional_checkpoint_id`. File bytes, canonical authorization, subscriptions,
watchers, renderer tickets, and renderer leases remain backend-owned.

The current Files contribution is reachable from Explorer and restoration but
is deliberately marked reserved in the New Surface catalog. Ordinary file
single-click uses `open_transient`; double-click, keyboard open, context
**Open**, and **Open to Side** create or pin permanent presentations. Artifact
resources, Draft/Changes, review, and active HTML/SVG isolation are not part of
this foundation, so activating the launcher would overstate the available
contract.

The Rust lifecycle is:

1. `open_file_resource` canonicalizes and authorizes the path, creates a
   subscription, and shares one watcher per canonical file.
2. A stable content change is debounced for 150 ms, becomes the next monotonic
   revision, and emits `file-resource://revision`. An unchanged hash emits
   nothing. Atomic replacement is accepted only when the original requested
   path still resolves to the same canonical target under the same authorized
   root. Persistent unreadable or unstable scans publish one typed unavailable
   revision and recover through the same revision stream.
3. Text reads are bound to the subscription and current revision. Image/PDF
   streams require a short-lived ticket bound to the subscription, WebView,
   renderer lease, and revision. Renderer calls carry the exact owning
   snapshot's `subscription_id`; the client never selects a newer subscription
   merely because another pane opened the same resource. Each issued ticket
   serves a verified immutable snapshot of that revision, so range reads do not
   reopen or rehash a changing source file.
4. `close_file_renderer_lease` revokes that renderer's tickets without closing
   another pane's subscription. `close_file_resource` releases one
   subscription; the watcher disappears only after the last subscriber closes.
   Ticket deadlines proactively reclaim abandoned snapshot storage and the
   matching lease. Issuance IDs prevent an older expiry task from revoking a
   newer ticket that reused the same renderer lease ID.

Every open and read rechecks current backend authority. A trusted restore with
no frontend capability selects a current matching agent primary workspace or
`include_directories`, in deterministic agent order, then an exact live picker
grant. `system_include_directories`, sibling paths, and canonical symlink or
junction escapes are denied. A saved Workbench document cannot restore access
that the backend no longer grants.

Current renderer limits are 16 MiB/200,000 lines for complete Monaco models,
5 MiB/100,000 lines per future diff side, 64 MiB/64 million decoded pixels for
images, and 256 MiB for PDFs. HTML and SVG resolve to the unsupported renderer
until the zero-Tauri-capability, networkless active artifact host is available.
PDF search is debounced and stops after 128 pages or two seconds, whichever
comes first. Partial results state exactly how many pages were searched; the
PDF renderer still mounts no more than three page slots.

## Persistence and Migration

Rust is the durable authority for:

- `<wardian-home>/settings/workbench.json`;
- `<wardian-home>/settings/workbench.backup.json`.

The frontend proposes a successor revision and passes the last acknowledged
revision plus an opaque backend token. Rust validates, serializes, hashes, and
atomically replaces the file. Concurrent writers use compare-and-swap rather
than structural auto-merge. A conflict freezes saving while preserving the
local draft for Use Disk, Replace Disk, or Export Local JSON.

On a fresh V1 home, Wardian performs one validated import from the legacy
`wardian-layout` local-storage record. It imports only clamped left/right
sidebar widths, bottom-terminal open/height state, and the initial surface. A
legacy Grid selection maps to Agents. The old record is removed only
after the exact migration save is acknowledged; resets or later saves cannot
accidentally acknowledge the import. Teams, watchlists, agent state, and domain
stores are not migrated into the workbench document.

Load order is primary, validated backup, then a usable default. A future schema
is preserved byte-for-byte and opened read-only with an in-memory fallback.
Corrupt or missing resources stay visible as placeholders with recovery
actions.

## Replaceable Layout Adapter and Safe Mode

`src/layout/workbench/DockviewLayoutAdapter.tsx` receives a read-only Wardian
document and emits Wardian commands. It may reconcile panels, groups, active
tabs, moves, drops, ratios, and zoom through Dockview's public APIs, but it must
not persist library-owned state or mutate the canonical model directly.

This boundary is intentionally replaceable: changing Dockview or building a
custom renderer must not require a persistence migration.

Set `WARDIAN_WORKBENCH_SAFE_MODE=1` before starting the native app to bypass
Dockview and render a simple one-group presentation. Safe mode still loads the
same document and surface registry. It does not flatten, rewrite, or discard
the durable split tree, so users can recover access without risking layout
data. Values other than the exact string `1` do not enable safe mode.

## Adding a Surface

1. Define bounded, versioned presentation state and decide which domain store
   owns the underlying resource.
2. Add the definition to the appropriate feature registry (the built-in set is
   assembled in `coreSurfaceRegistry.ts`) with explicit open, render, runtime,
   and close policies.
3. Implement strict default, serialize, and restore functions. Choose a
   `max_state_bytes` no larger than 64 KiB.
4. Provide command titles, accessibility labels, and dirty badges or guards as
   needed.
5. Render through the workbench host without importing Dockview into the
   feature module.
6. Test normal open, explicit duplicate, restore, version rejection,
   placeholder behavior, close guards, and hidden-renderer behavior.

## Verification Boundaries

- Pure Vitest tests prove command invariants, randomized sequences, registry
  validation, migration parsing, persistence queue behavior, and lifecycle
  guards.
- React integration tests prove surface rendering, tab/group commands,
  accessibility, close prompts, adapter projection, and safe layout behavior.
- Browser Playwright proves navigation, tabs, splits, responsive Overview,
  placeholders, and mocked restore. It cannot prove native disk or PTY claims.
- Native E2E proves Tauri persistence, exact primary/backup bytes, restart and
  corrupt/future-schema recovery, terminal leases, and that closing
  presentations leaves runtimes live.

See [Terminal Presentation Broker](./terminal-presentation-broker.md) for the
runtime contract used by Agent Session and Agents terminals.
