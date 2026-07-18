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
  Destructive navigation uses one two-phase resource close transaction before
  committing changes.
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
- optional resource-key and existing-instance resolution.

`confirm_if_dirty` definitions register a separate resource adapter. The
adapter reports canonical resource identity, monotonic generation, dirty state,
and deferred Save/Discard work. Keeping this outside `SurfaceDefinition`
prevents render components and presentation metadata from becoming close-time
authorities.

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
`file:<canonical-path>` or, in the future artifact slice,
`artifact:<artifact-id>`. Syntactic Windows absolute drive, UNC, and
extended-length paths normalize separators for identity; POSIX paths preserve
literal backslashes. Opaque artifact IDs remain verbatim. File and artifact
identities never collapse into each other even when an artifact is backed by
the same file.

An Explorer or restored path is provisional until `open_file_resource` returns
its backend-owned `resource_id`. The visible Files surface then applies
`canonicalize_resource` as one Workbench transaction. A normal alias open
focuses and removes itself in favor of an existing canonical presentation,
even when that presentation is in another pane. Only `open_to_side` carries
strictly validated `presentation_provenance` through the rekey of both it and
its matching source presentation. This generic Workbench record is persisted
with the surface, so restoring the app between either backend response does not
collapse an intentional duplicate. The pair converges to the same canonical
key and remains in separate panes regardless of which response publishes first.
Repeated same-key acknowledgements do not consume this provenance while the
matching presentation still has its provisional key.
Once both endpoints settle, the record is removed atomically without removing
the duplicate. An accepted explicit resource rebind, close, group close, reset,
or missing endpoint detaches the affected pair; cancelled or stale operations
leave the original provenance intact. Legacy documents without the optional
field remain valid, while unknown record keys and cross-surface owner IDs are
rejected.
The frontend never resolves symlinks, junctions, or filesystem authority.
Canonicalization re-resolves the current Workbench document after a stale
layout compare-and-swap. A cancellation remains a user veto. A transaction or
resource membership change while a dirty-resource choice is pending cancels
before Save/Discard effects, rather than replaying a choice against new state.

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

`close_view` can close immediately. `confirm_if_dirty` participates in a batch
resource transaction. Navigation captures an immutable document, transaction
version, and complete closing-surface set; the registry groups exact
presentations by canonical resource and prepares each final-closing dirty
resource once. It collects every choice before effects, revalidates transaction,
membership, identity, and generation, runs all requested Saves, commits layout
once, then runs Discard/release cleanup. A cancel, stale preparation, or failed
save performs no layout commit and no discard.

Library resources remain presentation-keyed because their editor bridges are
presentation-owned. The Library store publishes a monotonic generation for
every draft, baseline, and entry-identity change, even when the dirty boolean
does not change. Workflows uses one shared builder resource across every
presentation and advances a monotonic resource revision on load, initialize,
edit, save, discard, and reset. Prepared Save and Discard effects recheck their
observed identity and generation immediately before invoking the resource
action; stale effects fail closed. Files registers its shared editor-controller
adapter. The adapter snapshots buffer generation and canonical resource
identity so close-time Save or Discard cannot act on a replaced editor session.

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

`FilesSurfaceStateV2` serializes resource kind, transient-preview state,
rendered/editor presentation, comparison preferences, and review identifiers.
Restored V1 state is normalized at the surface boundary. File bytes, editor
buffers, dirty state, canonical authorization, subscriptions, watchers,
renderer tickets, and renderer leases remain outside the Workbench document.

Each tab persists its rendered/editor choice, but presentation never changes
subscription ownership. Text panes that share a canonical resource use one
`FileEditorController` and one Monaco model URI keyed by backend `resource_id`.
Every pane receives its own editor view over that model; revision refreshes do
not recreate the model or discard undo history. The controller registry owns
model lifetime and disposes it only after the final presentation and durable
recovery hold are released.

The current Files contribution is reachable from Explorer and restoration but
is deliberately marked reserved in the New Surface catalog. Ordinary file
single-click uses `open_transient`; double-click, keyboard open, context
**Open**, and **Open to Side** create or pin permanent presentations. The first
buffer mutation also pins a transient source-only file. The shipped comparison
surface uses the Saved-file baseline; prompt-checkpoint and presented-version
baselines remain explicit unavailable variants until downstream adapters supply
their exact historical content. Files never falls back to a nearby revision or
adds top-level Preview/Changes/Draft modes.

The Files header contains a current-state Book/Pencil presentation control only
when both rendered and editor presentations exist. Source-only resources open
directly in Monaco. Dirty state appears as generic presentation badges in both
Dockview and safe-layout tabs and as a dot beside the Explorer-safe breadcrumb;
the title string is never changed with `*`. Save is explicit through
Ctrl/Cmd+S or the overflow menu. Save As consumes a one-shot exact native picker
grant, writes the working buffer, then opens the returned canonical path as a
new ordinary file. It never retargets the source controller or artifact identity.

The controller persists a recovery record outside the Workbench document for
each dirty canonical resource. A restart may reconstruct dirty, stale, or
read-only recovery state. The same Wardian app webview may read the exact saved
recovery base and buffer without current file authorization so the user can
inspect or discard them, but that recovery grants no access to current disk
bytes. Restoring write access, merging against disk, or saving still requires a
newly authorized live subscription for the exact target. The final presentation
close uses the ordinary shared-resource **Save** / **Don't Save** / **Cancel**
transaction. There is no review-owned draft model or second editor buffer.

`FileComparisonLens` keeps comparison presentation separate from the working
buffer. The editor controller keeps `buffer_base_hash` and `disk_head_hash`
independent; Saved-file comparison uses the former. Downstream historical
baseline adapters must supply a separate immutable `review_base_hash` without
changing either editor hash. Saved-file changes are annotated inline and
summarized with a compact changed-region count. The lens selects side-by-side
layout at 720 px of available pane width and unified layout below it. An
explicit side-by-side preference remains effective to a 560 px hard minimum;
narrower panes receive a temporary unified override without mutating that
preference. Review-drawer width is removed before applying either threshold.

When a watcher publishes a new disk head while the controller is dirty, the
controller enters stale state. **Merge** rebases the shared buffer and leaves it
dirty until explicit Save, **Reload from disk** discards the buffer and recovery
record and adopts the new disk head, and **Cancel** preserves the stale state.
All three actions validate the same canonical controller identity and observed
generation before taking effect.

The reserved downstream artifact integration keeps `artifact:<id>` as its
durable provenance/navigation identity. Its attachment adapter must resolve the
currently presented backing version to an authorized canonical file or
immutable blob controller and then attach the Files presentation to that
controller. Duplicate file and artifact presentations therefore share one
editor model when they resolve to the same canonical file; artifacts do not own
a second buffer. **Save As** always creates an ordinary file presentation and
never retargets the artifact thread.

The Rust lifecycle is:

1. `open_file_resource` canonicalizes and authorizes the path, creates a
   subscription, and shares one watcher per canonical file. Each subscription
   retains its own access claim and exact requested-path authorization; the
   shared resource never lends one subscriber's provenance to another. Joining
   an existing resource schedules availability reconciliation even when no file
   event occurred.
2. A stable content change is debounced for 150 ms, becomes the next monotonic
   revision, and emits `file-resource://revision`. An unchanged hash emits
   nothing. Atomic replacement is accepted only when the original requested
   path still resolves to the same canonical target under the same authorized
   root. Refresh tries active subscription authorizations in deterministic
   requested-path and subscription-ID order. A failed alias candidate cannot
   poison a valid direct candidate, and a typed unavailable revision is
   published only when every active candidate fails. Persistent unreadable or
   unstable scans recover through the same revision stream. Before any content
   scan, agent candidates resolve their live configuration from backend
   `AppState`, picker candidates prove their exact capability is still live,
   and both revalidate the subscription's original requested pathname.
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
   A close that leaves subscribers schedules reconciliation so removing the
   last valid candidate cannot leave an invalid-only resource marked available.
   Ticket deadlines proactively reclaim abandoned snapshot storage and the
   matching lease. Issuance IDs prevent an older expiry task from revoking a
   newer ticket that reused the same renderer lease ID.

Every open and read rechecks current backend authority. A trusted restore with
no frontend capability selects a current matching agent primary workspace or
`include_directories`, in deterministic agent order, then an exact picker path
from the backend-owned durable registry. A durable path match mints a fresh
live capability and retained handle. `system_include_directories`, sibling
paths, and canonical symlink or
junction escapes are denied. A saved Workbench document cannot restore access
that the backend no longer grants. If an alias and a direct pathname share one
canonical resource, removing or retargeting the alias revokes only subscriptions
opened through that alias; direct subscriptions retain reads, tickets, refresh,
and the shared watcher. Picker-grant eviction uses grant-local in-flight and
active-subscription counts under one mutex, so a concurrent open cannot become
evictable between a stale membership snapshot and grant selection.

Current renderer limits are 16 MiB/200,000 lines for complete Monaco models,
5 MiB/100,000 lines per diff side, 64 MiB/64 million decoded pixels for
images, and 256 MiB for PDFs. HTML and SVG are editor-only inert source in
Monaco. They are never injected into Wardian's DOM; a future active artifact
host must provide the zero-Tauri-capability, networkless isolation contract
before either type can render live.
PDF search is debounced and stops after 128 pages or two seconds, whichever
comes first. Partial results state exactly how many pages were searched. The
PDF renderer mounts a dynamic bounded window whose radius adapts to the
viewport from one to 12 pages on either side of the center page, for an exact
maximum of 25 mounted page slots.

A byte- or decoded-pixel-limit violation is a stable metadata revision, not an
open failure. Its descriptor keeps the canonical name, type, byte size, and
modified time but disables content rendering, comparison, editing, and stream
capabilities.
Exact readable scans use `sha256:<full-content-digest>` as their revision
identity. Metadata-only scans use `bounded-sha256:<revision-fingerprint>`,
derived from retained file identity, stable write metadata, and a bounded
leading probe. The bounded form is never presented as a full-content hash and
its opaque token cannot authorize reads or tickets. Watcher refresh compares
the bounded identity, suppresses unchanged oversized scans, and advances when
the oversized revision changes or recovers within its renderer limit.

Browser E2E proves Files rendering, controls, annotations, comparison layout,
and stale-choice wiring only against mocked IPC. Native E2E is the authority
for picker and root authorization, retained file handles, atomic filesystem
writes, real watcher conflicts, durable recovery after runtime recreation, and
subscription or renderer-lease cleanup. Browser evidence must not be used to
claim those native properties.

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
