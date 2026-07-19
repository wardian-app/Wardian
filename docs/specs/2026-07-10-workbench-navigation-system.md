# Workbench Navigation System

- **Status:** Approved
- **Date:** 2026-07-10
- **Epic:** [#513](https://github.com/wardian-app/Wardian/issues/513)
- **Supersedes:** `docs/specs/2026-06-07-navigation-layout-v2.md`

## Context and Problem Statement

Wardian currently selects one of seven global main views from a fixed titlebar
control. That model is easy to understand while every capability is a page, but
it cannot support the product Wardian is becoming: several agent sessions in
parallel, an agent beside Queue or Garden, file and browser tools, and
extension-contributed work surfaces.

The replacement must feel like a development workbench rather than a new set of
pages. Users need to open durable work objects, place them in tab groups or
splits, move among them quickly, and recover the exact arrangement after a
restart. Wardian's persistent left tool rail and right agent roster remain
valuable orientation and control regions; they should not be mistaken for the
main-surface launcher.

The navigation system is also inseparable from terminal geometry. An agent can
appear in Agents Overview, a dedicated Agent Session surface, and a remote
client at the same time. Letting each renderer resize the same PTY produces
unstable dimensions and broken desktop rendering. A workbench is only smooth if
terminal ownership and mirroring are first-class runtime contracts.

This spec replaces the exploratory Site/Cohort/Perspective design. Teams and
watchlists remain independent agent-organization concepts. Wardian can later
offer named workbench contexts, but the first implementation does not create a
second hierarchy of Sites or couple layout state to an unproven Cohort model.

## Goals

1. Replace fixed global main-view selection with pane-local surface tabs and
   splits, comparable in feel to VS Code, Obsidian, and cmux.
2. Preserve Wardian's shell: left auxiliary-tool rail, optional left pane,
   persistent right roster, bottom user-terminal dock, and modal settings.
3. Make Agents Overview a first-class multi-agent surface with responsive
   Auto, Grid, and Single presentation modes.
4. Support multiple simultaneous presentations of an agent terminal without
   geometry fights, lost output, or ambiguous input ownership.
5. Restore the last workbench exactly, while keeping runtime truth and layout
   state cleanly separated.
6. Establish a typed surface registry so file editors, browsers, and extension
   surfaces can be added without changing the shell architecture.
7. Keep layout state inspectable, versioned, recoverable, and local-first under
   `WARDIAN_HOME`.
8. Make keyboard navigation, accessibility, responsive behavior, and
   performance part of the base contract rather than follow-up polish.

## Non-Goals

- Defining Sites, Cohorts, or Perspectives as foundational product domains.
- Renaming or migrating Teams and watchlists as part of navigation work.
- Shipping the file editor or agent browser in the first navigation cutover.
- Making auxiliary panels arbitrary draggable central surfaces.
- Moving the bottom user terminal into the central workbench in the first
  release.
- Persisting terminal contents, live agent/runtime state, focus DOM nodes, or
  credentials in the workbench document.
- Allowing layout changes or tab closure to start, stop, kill, or transfer
  ownership of agent runtimes implicitly. The current interactive owner may
  issue serialized, coalesced geometry updates when its pane is deliberately
  resized or zoomed; mirrors may not.

## Product Model

### Workbench and shell

```text
AppShell
├── CustomTitleBar             telemetry, drag space, side/window controls
├── LeftControlRegion
│   ├── SidebarIconRail        auxiliary-tool selector only
│   └── SidebarContentPane     contextual tool content
├── WorkbenchController
│   ├── WorkbenchModel         groups, tabs, split tree, active surfaces
│   ├── SurfaceRegistry        type contracts and lifecycle policy
│   ├── NavigationService      open, focus, move, split, close, quick open
│   ├── WorkbenchPersistence   versioning, atomic save, migration, recovery
│   └── LayoutAdapter          third-party renderer boundary
├── AgentRoster                persistent right-side monitoring and routing
├── UserTerminalDock           existing bottom dock
└── SettingsDialog
```

`App.tsx` remains the root composition point, but it must stop owning every
view's switching and state rules. Global resource stores live outside surface
instances. Each surface owns only its presentation state.

### Navigation boundary

The fixed `Grid / Dashboard / Queue / Graph / Garden / Library / Workflows`
titlebar list is removed.

The left icon rail does **not** replace it. Its current responsibility remains
selecting auxiliary tools such as Explorer, Source Control, agent configuration,
commands, workflows glance, terminal, and settings. An action on an object in an
auxiliary pane may open or focus a central surface, but the rail itself does not
switch the global view.

The right roster remains persistent, but target selection and navigation are
separate. Single-click and multi-select update the global command/broadcast
target set only. Double-click, Enter, or an explicit Open action opens or focuses
that agent's Agent Session surface. Open to Side creates another presentation in
a neighboring group. The roster target set is global domain UI state; the active
Agent Session and the focused agent inside an Agents Overview surface are
presentation state and never overwrite the target set implicitly.

Central surface discovery uses:

- a `+` button in every group tab strip that creates an inline New Tab;
- Home / New Surface in an empty group;
- Quick Open;
- the command palette;
- contextual links and object actions from auxiliary tools and surfaces;
- recent surfaces and typed registry contributions.

There is no mandatory Home hop between normal tasks.

The titlebar is intentionally quiet. It does not expose persistent surface or
command buttons. Each pane owns a browser-like tab strip; its persistent
controls are limited to New Tab (`+`) and More Actions (`…`). Tab-specific
actions live in the tab context menu, pane structure actions live in the pane
menu, and the searchable Quick Open and command palettes appear only while
invoked.

### Groups, tabs, and splits

Each split group owns an independent ordered tab stack and active tab. A surface
can be moved within or between groups, or placed beside another surface by
drag-and-drop or command. Splits form a normalized binary tree with horizontal
or vertical orientation and proportional sizes.

Group zoom temporarily expands one group to the workbench bounds. It is
reversible and does not mutate or persist the underlying split tree.

Close Group evaluates every tab's close guard before mutating anything. Any
cancel leaves the complete group/tree unchanged. On confirmation, its surfaces
close in tab order and enter `recently_closed`, the sibling subtree replaces the
removed split node, and that subtree's leftmost depth-first group becomes active.
Closing the only group closes its tabs but retains that
empty group so Home appears. Moving all tabs into an adjacent group is a
separate Join Group command that preserves their order; Close Group never
silently redistributes them.

Closing a tab closes a presentation. Closing the last Agent Session
presentation never kills the agent; lifecycle actions remain explicit.

Normal open follows resource-aware focus behavior:

- singleton surfaces focus their existing instance;
- `agent-session` focuses the most recent presentation for that agent;
- explicit Open to Side or duplicate creates a second presentation;
- resource-keyed editor/browser contracts may choose focus or duplicate based
  on their registry policy.

## Surface Model

### Stable identifiers

- `surface_id`: one persisted presentation instance.
- `surface_type`: registry key for rendering and lifecycle behavior.
- `resource_key`: optional stable domain identity, such as an agent UUID.
- `group_id`: group containing the surface presentation.
- `runtime_id`: optional live runtime identity; never the persistence identity.

Layout placement belongs to groups. Surface view state belongs to surface
instances. Domain/runtime state belongs to shared stores and the Rust backend.

### Surface registry contract

```ts
type SurfaceRenderPolicy =
  | "keep_alive"
  | "suspend_when_hidden"
  | "recreate_from_state";

type SurfaceOpenPolicy =
  | "singleton"
  | "focus_resource"
  | "allow_multiple";

type SurfaceRuntimePolicy = "view_only" | "runtime_backed";

type SurfaceClosePolicy = "close_view" | "confirm_if_dirty";

type SurfaceDefinition<TState extends SurfaceState = SurfaceState> = {
  type: SurfaceType;
  title: (surface: SurfaceInstance<TState>) => string;
  icon: SurfaceIcon;
  render_policy: SurfaceRenderPolicy;
  open_policy: SurfaceOpenPolicy;
  runtime_policy: SurfaceRuntimePolicy;
  close_policy: SurfaceClosePolicy;
  state_schema_version: number;
  max_state_bytes: number;
  default_state: () => TState;
  resource_key?: (request: OpenSurfaceRequest) => string | undefined;
  resolve_existing?: (
    request: OpenSurfaceRequest,
    candidates: SurfaceInstance<TState>[],
  ) => string | undefined;
  serialize_state: (state: TState) => SerializedSurfaceState;
  restore_state: (value: unknown) => SurfaceRestoreResult<TState>;
  can_close?: (surface: SurfaceInstance<TState>) => CloseDecision;
  commands: SurfaceCommandDefinition[];
  badges?: (surface: SurfaceInstance<TState>) => SurfaceBadge[];
  render: SurfaceRenderer<TState>;
};
```

The registry is Wardian-owned. It validates restored payloads, supplies missing
surface placeholders, resolves normal-open versus explicit-duplicate behavior,
exposes commands and accessibility metadata, and defines close guards.
Third-party layout code never becomes the canonical model.
Registry-owned presentation synchronization may feed lightweight metadata from
the current document into a contribution, but the raw mutation callback remains
internal: `get`, `require`, and `list` expose only guarded public definitions.

Render policies are executable contracts:

- `keep_alive`: keep the surface component mounted and subscribed while its tab
  is hidden; expensive child renderers may still yield to a declared global
  resource budget.
- `suspend_when_hidden`: retain logical presentation and serialized view state,
  but pause subscriptions and destroy expensive canvas/xterm renderers after a
  30-second hidden grace period; reconstruct from shared runtime/state on show.
- `recreate_from_state`: unmount hidden content immediately and recreate it from
  registered state when shown.

The terminal renderer budget allows at most 24 mounted xterm renderers and 12
WebGL contexts process-wide. Visible presentations have priority, then the most
recently interacted hidden presentations. Over-budget hidden renderers suspend
least-recently-used without unregistering their logical presentations. Agents
Overview keeps its surface controller alive, but its hidden cards obey the same
terminal budget. The Phase 0 proof includes a 20-tab/four-group case and an
Overview dense enough to exercise both limits.

Home is derived UI for an empty group and is not persisted. A user-created New
Tab is different: it is a registered, allow-multiple placeholder surface that
renders the same launcher with ordinary tab identity. Choosing a surface
replaces that placeholder atomically at its existing group and index. Focusing
an already-open singleton or reopening recent work discards only the New Tab;
the internal discard command rejects every non-New-Tab surface and does not add
the placeholder to `recently_closed`.

### Initial taxonomy

| Surface | Open policy | Render policy | Runtime policy | Notes |
|---|---|---|---|---|
| New Tab | allow multiple | recreate from state | view only | Inline launcher; excluded from surface discovery |
| Agents | singleton | keep alive | view only | Existing Grid evolved into responsive multi-agent monitoring |
| Dashboard | singleton | recreate from state | view only | Aggregate telemetry |
| Queue | singleton | recreate from state | view only | Signals and action-needed state |
| Graph | singleton | suspend when hidden | view only | Heavy visualization keeps registered view state |
| Garden | singleton | suspend when hidden | view only | Heavy visualization keeps registered view state |
| Library | singleton initially | keep alive | view only | Preserves selection and dirty-aware editor state |
| Workflows | singleton initially | keep alive | view only | Preserves builder/monitor state |
| Agent Session | focus resource; explicit duplicate | suspend when hidden | runtime backed | Runtime survives all presentations |

The registry reserves typed contracts for File Editor and Browser so they can
be contributed later. Their actual views are outside this implementation.

### Agents modes

Agents exposes `Auto`, `Grid`, and `Single`.

Its population is the current global active roster/watchlist filter, matching the
agents the right roster exposes. Search and lifecycle/status filters may narrow
that population. The global `selected_agent_ids` set only highlights and targets
commands; it never adds/removes Overview cards and is not the focused agent.
Plain roster click selects one target, Ctrl/Cmd-click toggles a target,
Shift-click extends the stable visible range, and clicking empty roster space
clears the target set. Double-click/Enter/Open performs navigation after the
selection gesture. Each Agents Overview surface persists its own
`focused_agent_id` and presentation mode.

- `Auto` is the default. It chooses Grid or Single from the surface's actual
  container size, current Overview population count, and minimum usable terminal dimensions.
- `Grid` lays out the current Overview population and chooses rows/columns to
  maximize terminal usability.
- `Single` is the evolution of the current maximize behavior: one focused agent
  fills the surface while the workbench layout and other runtimes remain intact.
- A user override is persisted in the Agents Overview surface state. `Auto`
  may change as the surface is resized; explicit Grid or Single does not.
- The mode affects presentations only. It never opens/closes Agent Session tabs,
  kills agents, or changes remote/terminal ownership by itself. Cards hidden by
  Single remain logical hidden presentations; a mode transition cannot activate
  or promote them.

The automatic layout scores candidate grids using measured content bounds,
terminal cell metrics, required chrome, and minimum usable card bounds. The
initial floors are 520 x 280 CSS pixels for a terminal card and 360 x 280 for a
chat card, including 52 pixels of card chrome. These values are named constants
and are recalibrated from the Phase 0 cell-metric baseline rather than hidden in
CSS.

`Auto` evaluates useful viewport capacity rather than requiring the entire
roster to fit at once. If at least two floor-sized cards fit side by side, it
selects a multi-column grid, preserves the card floor, and allows additional
rows to scroll vertically. It selects Single only for one visible agent or when
the pane is too narrow for two useful cards. Resize decisions are debounced for
120 ms and require either crossing a hard floor or a 10% score improvement over
the current choice, preventing split-drag flicker. Explicit Single remembers
the last explicit `Auto` or `Grid` mode and Minimize restores that mode; an
Auto-derived singleton is not presented as user-maximized.

Explicit Grid never silently becomes Single. If the requested grid cannot meet
the floor, cards retain minimum dimensions and the surface scrolls. If the
focused Single agent disappears, Wardian selects the most recently interacted
remaining agent, then the first stable agent order; no agents renders the
standard empty state. Breakpoints based only on the application window are
forbidden because splits and sidebars change usable space independently.

## Terminal Session Broker

### Invariants

1. A PTY has at most one interactive geometry/input owner at a time.
2. Every visible representation receives live output.
3. Only the current owner can submit PTY input or resize requests.
4. Passive mount, restore, `ResizeObserver`, and DOM focus cannot acquire
   ownership.
5. Ownership changes through an explicit user interaction or activation
   command. Automatic promotion after owner loss is the sole broker-driven
   exception.
6. Moving, splitting, hiding, or closing a presentation does not recreate the
   runtime.
7. Remote and desktop presentations use the same broker contract.

### Presentation registration

Each logical terminal presentation registers independently of its renderer
mount. Agent Session surfaces use their `surface_id`; Agents Overview derives a
stable ID from its surface and agent IDs; remote clients use the authenticated
attachment ID. Suspending/unmounting a renderer updates the presentation to
hidden/suspended but does not destroy it or transfer ownership. Unregistering
happens only when the logical presentation is removed or the client disconnects.

Registration sends client-owned fields:

```ts
type TerminalPresentationRegistration = {
  presentation_id: string;
  session_id: string;
  client_kind: "desktop" | "remote";
  desired_geometry?: { rows: number; cols: number };
  visibility: "visible" | "hidden";
  render_state: "mounted" | "suspended";
  requested_interaction: "interactive" | "read_only";
  observed_lease_epoch: number;
};
```

The Rust backend is authoritative for session lifetime, lease owner, lease
epoch, canonical PTY geometry, an ordered terminal stream sequence, and a
monotonic interaction sequence returned in broker state. The frontend keeps renderer state and
presentation-local viewport state only.

`requested_interaction` can only downgrade capability. The broker derives the
effective `interaction_capability` from trusted local-app identity or the
authenticated remote ticket, device policy, and client kind. A remote client
cannot self-grant input/activation rights by changing registration JSON.

Every registered presentation may call
`report_terminal_presentation_viewport(session_id, presentation_id,
runtime_generation, cols, rows)`.
This clamps and stores non-authoritative desired geometry without touching the
PTY. If that presentation later activates or is promoted, the broker uses the
freshest report. Only the active owner's separate epoch-bearing geometry command
may resize the PTY.

### Ownership transfer

Ownership has three states: no owner, one active owner, or one pending transfer
that retains the previous owner only as a rollback candidate. Activation is a
two-phase protocol:

1. `begin_terminal_activation(session_id, presentation_id,
   runtime_generation, observed_lease_epoch)` validates an interactive-capable presentation,
   increments the lease epoch, records a pending transfer, and rejects input and
   resize from both old and proposed owners for the short transfer window.
2. The broker returns `lease_epoch`, `activation_id`, `runtime_generation`, a
   fresh bounded snapshot, and its `sequence_barrier`. Output after the barrier
   remains available through the replay ring while the client applies it.
3. `ack_terminal_activation(session_id, presentation_id, runtime_generation,
   lease_epoch, activation_id)` validates the pending transfer, assigns the new active owner,
   applies at most one valid desired geometry, broadcasts ownership/geometry,
   and enables input.

If acknowledgement does not arrive within five seconds, the broker cancels the
pending transfer. A still-eligible previous owner becomes active at the new
epoch; otherwise the session has no owner. Duplicate begin/ack requests are
idempotent for the same activation ID, while a newer activation supersedes an
older pending request. Tests pin begin/begin, ack/ack, old-ack-after-new-begin,
disconnect-during-pending, and timeout races.

Every input and resize request includes `session_id`, `presentation_id`,
`runtime_generation`, and `lease_epoch`. Resizes also carry a monotonically increasing
`geometry_sequence`. Stale identity, epoch, or geometry sequence is rejected
without disconnecting the client. The client refreshes ownership state and
remains a live mirror.

The active owner's `ResizeObserver` may propose geometry when its pane is
deliberately resized or zoomed. The frontend coalesces proposals to an animation
frame; the backend serializes last-write-wins PTY resizes and rejects reordered
requests. Desktop geometry is clamped to 20..500 columns and 8..200 rows. The
remote adapter retains its stricter 20..240 and 8..80 limits. Mirrors perform
local fitting only.

If the owner disconnects, the broker promotes the eligible presentation with
the highest server-owned interaction sequence and applies its last valid
geometry. Eligibility requires registration, visibility, interactive
capability, a mounted renderer, and a live runtime. If none exists, the PTY
retains its canonical dimensions with no owner until the next explicit
activation. Client wall-clock
timestamps never decide ownership.

First-owner bootstrap is explicit: the user action that spawns an agent, opens
an Agent Session, activates an Agents Overview card, or opens a remote terminal
starts the same two-phase activation. Restored/mounted terminals remain mirrors
until a user activates one.

### Mirror rendering and output

Each presentation owns its own xterm renderer. Reparenting one module-global
renderer among DOM hosts is not allowed.

Renderer retirement is lease-bound. Output, reset, refresh, and follow-up scroll
work capture one renderer generation before awaiting; retirement immediately
releases its budget slot but defers physical disposal until every in-flight
operation releases its lease. Post-await work may mutate only that captured,
still-current renderer. Agents scrolling changes WebGL priority independently
and does not destroy resident xterm renderers within the process budget.

Each broker runtime owns a monotonically increasing `runtime_generation` and
`stream_sequence`. A fresh generation starts at sequence zero. One per-session
control/parser lock orders raw PTY output, canonical geometry changes, ownership
barriers, and generation boundaries. Every non-empty PTY read and every
committed geometry change advances the sequence and writes an event to a replay
ring capped at both 4,096 events and 1 MiB of raw bytes; the older bound wins.
Registered hidden or unmounted presentations do not receive private queues and
therefore cannot grow memory without bound.

A hidden owner remains the owner without changing canonical geometry, but input
is disabled while its renderer is suspended. On remount it requests a fresh
snapshot/barrier and acknowledges application before input resumes at the same
generation/epoch, unless another explicit activation won ownership meanwhile.
Renderer-budget eviction uses this same resync path. Tests cover hide, budget
eviction, remount, concurrent takeover, and stale resync acknowledgement.

A geometry commit applies the native resize and VT parser resize in the same
serialized operation, then emits a geometry event and snapshot barrier before
later output. Mirrors never infer ordering from arrival time. Tests race output
against resize and require owner and mirrors to converge on identical geometry,
screen contents, and final stream sequence.

A snapshot contains the canonical visible grid, terminal modes/attributes, and
up to 1,000 lines of bounded scrollback, plus geometry, generation,
`snapshot_id`, and `sequence_barrier`. Its serialized payload is capped at 2 MiB.
A presentation applies the snapshot, discards events at or below the barrier,
then replays consecutive later events. A generation change, missing sequence,
broadcast lag, or expired replay range requests a new snapshot. Slow clients are
resynchronized instead of accumulating an unbounded send backlog. Remote socket
backpressure and rate limits remain in force.

Feed consumers are separate from interactive presentations. One desktop session
consumer fans the shared ordered stream to local renderers; each authenticated
remote socket has its own cursor. Consumers subscribe with a generation, pull
bounded batches (`after_sequence`, at most 256 events / 256 KiB), acknowledge
applied sequences for lag diagnostics, and unsubscribe explicitly. A cursor gap,
generation change, or termination is a structured batch status; gap/generation
responses carry a recovery snapshot and barrier. Acknowledgements do not pin
replay retention and no consumer receives a private output queue. Desktop is
woken by a coalesced session/sequence event; remote sockets use broker wake-ups
and the same cursor protocol.

Mirrors never resize the PTY. They render the canonical owner grid:

- fit locally at normal font size when possible;
- reduce local scale only to an accessibility/readability floor;
- pan when the canonical grid still does not fit;
- letterbox when the mirror is larger than the canonical grid.

Wardian does not use smallest-client-wins geometry. That strategy lets a phone
or tiny split degrade every desktop terminal and is incompatible with stable
multi-surface rendering.

### Compatibility migration

The broker first lands behind the existing terminal UI so terminal correctness
can be proven independently of the workbench. Existing remote attach commands
remain compatible during one migration window, implemented as adapters to the
same lease model. There is one canonical writer for ownership and geometry.

The adapter must preserve authenticated attachment/device identity, per-agent
connection limits, warm detach and generation cleanup, input size/rate limits,
socket backpressure, geometry validation, and the current owner guard. A stale
or non-owner input/resize becomes a nonfatal lease-state response; it must not
close an otherwise healthy remote socket. Unit and native tests pin every one of
these no-regression contracts before the compatibility path is removed.

Desktop keystroke/binary-input and resize commands are replaced by
presentation-aware commands carrying session, presentation, and epoch. The old
`send_input_to_agent`, `send_binary_input_to_agent`, and
`resize_agent_terminal` commands are legacy adapters for one migration window;
the workbench never calls them, and they are retired at cutover. Structured
prompt delivery, command broadcast, and backend-authorized injection remain
separate audited runtime operations because they intentionally target an agent
without pretending to be terminal keystrokes.

The broker is created and destroyed with the PTY runtime, not with
presentations. With zero presentations it retains canonical geometry and bounded
parser/replay state until the runtime ends. Kill terminates the broker and
notifies subscribers without pruning persisted tabs. Pause retains broker state
but marks runtime unavailable. Resume, clear, or PTY replacement increments
`runtime_generation`, increments the lease epoch, revokes owner/pending transfer,
resets sequence/parser/replay state, and forces every presentation through a
fresh activation snapshot before input resumes. A stale presentation therefore
cannot write to a replacement PTY that reused the same session ID.

## Persistence and Recovery

### Canonical `WorkbenchDocumentV1`

The durable model is a Wardian DTO, not layout-adapter output:

```ts
type WorkbenchDocumentV1 = {
  schema_version: 1;
  revision: number;
  saved_at: string;
  root: WorkbenchNodeV1;
  groups: Record<string, WorkbenchGroupV1>;
  surfaces: Record<string, WorkbenchSurfaceV1>;
  active_group_id: string;
  recently_closed: ClosedSurfaceV1[];
  shell: WorkbenchShellV1;
};

type WorkbenchNodeV1 =
  | { kind: "group"; group_id: string }
  | {
      kind: "split";
      node_id: string;
      direction: "horizontal" | "vertical";
      ratio: number;
      first: WorkbenchNodeV1;
      second: WorkbenchNodeV1;
    };

type WorkbenchGroupV1 = {
  group_id: string;
  surface_ids: string[];
  active_surface_id: string | null;
};

type WorkbenchSurfaceV1 = {
  surface_id: string;
  surface_type: string;
  resource_key?: string;
  presentation_provenance?: {
    kind: "explicit_duplicate";
    duplicate_surface_id: string;
    partner_surface_id: string | null;
    provisional_resource_key: string;
  };
  state_schema_version: number;
  state: unknown;
};

type ClosedSurfaceV1 = {
  surface: WorkbenchSurfaceV1;
  previous_group_id: string;
  previous_index: number;
};

type WorkbenchShellV1 = {
  left_sidebar_collapsed: boolean;
  left_sidebar_width: number;
  right_sidebar_collapsed: boolean;
  right_sidebar_width: number;
  bottom_terminal_open: boolean;
  bottom_terminal_height: number;
};
```

Groups may be empty; an empty group derives Home / New Surface. Every group is
referenced exactly once by the tree. Every open surface is referenced exactly
once by one group. Active IDs are present in their corresponding collections,
split node IDs and surface/group IDs are unique, split ratios are in
`0.1..0.9`, and the tree is acyclic. The default document contains one empty
active group.

The model command layer validates preconditions and the full post-state before
publishing a mutation. Failed commands leave the previous document unchanged.
`recently_closed` keeps at most 20 registry-approved snapshots and powers Reopen
Closed Surface. Reopening restores the prior group/index when possible and
falls back to the active group; an ID collision generates a new `surface_id`.
Presentation provenance is open-layout lifecycle intent, not historical state.
The model strips it when recording or reopening a closed surface, including
legacy persisted history and collision-renamed reopens.

The entire document is limited to 2 MiB, split-tree depth to 64 nodes, and each
registered surface state to 64 KiB. A registry may set a smaller limit. Unknown surface types are preserved
as inert opaque JSON, including their `state_schema_version`, within the same
limit so reinstalling a contribution can recover them. Wardian never executes
or merges unknown state before the type is registered and validates it.

### Storage boundary

The last workbench is stored as versioned JSON at
`<wardian-home>/settings/workbench.json` by atomic Rust commands. Its
last-known-good backup is
`<wardian-home>/settings/workbench.backup.json`. This follows Wardian's existing
device-local UI/settings boundary while remaining directly inspectable.

Writes use same-directory temp-file + file flush + atomic replace and flush the
parent directory where the platform requires it (`MOVEFILE_WRITE_THROUGH` on
Windows; directory `sync_all` after rename on Unix). The backup rotates only
from a successfully parsed and fully validated primary; a corrupt primary can
never replace the last-known-good file. Backup replacement is made durable
before primary replacement, and acknowledgement follows the final durable
primary replace. The backend rejects invalid schemas, unsafe sizes, and
non-monotonic revisions.

Workbench mutations enter one serialized save queue immediately. If a write is
active, later mutations coalesce to the newest revision; no acknowledged newer
revision is overwritten by an older one. The UI considers a revision durable
only after the backend acknowledges it and exposes a subtle unsaved indicator
while a write is pending. Normal-operation coalescing has a 250 ms maximum
window, and explicit Reset/Restore commands flush immediately. Clean shutdown
requests a final flush but is not the sole durability mechanism. “Exact restore”
means the latest acknowledged revision; a process or power crash can lose only
the currently unacknowledged window.

Revision zero is the uninitialized base. The frontend proposes exactly
`durable_revision + 1`; the backend accepts it only when the supplied expected
revision and opaque durable token equal disk, stays within JavaScript's maximum
safe integer, and returns the durable revision/token plus the caller's request
ID. Only Rust serializes and hashes the exact persisted bytes; TypeScript treats
the token as opaque. A lost-response retry is idempotent when its revision and
backend-computed token equal the durable primary; same-revision/different-content
is a conflict. Local
mutations made during a write remain in a separate working draft and become the
next revision only after acknowledgement. A CAS conflict freezes saving and
preserves the complete local draft. Wardian never structurally auto-merges two
workbench documents: the user explicitly chooses Use Disk, Replace Disk after a
fresh CAS rebase/validation, or Export Local JSON. A future-schema conflict is
export-only.

The document is device-local UI state. It contains:

- the fields defined by `WorkbenchDocumentV1`;
- the Agents Overview presentation override inside its registered state.

It excludes:

- agent/provider/runtime truth;
- PTY contents, stream sequence, owner lease, or geometry;
- credentials, tokens, and unbounded or executable extension state;
- group zoom;
- transient hover/drag/DOM focus state;
- recomputable data already available from domain stores.

### Restore behavior

Startup loads and validates the primary document, then the backup. If both fail,
Wardian opens a usable default workbench with Home / New Surface and reports a
non-blocking recovery notice. If a binary encounters a newer schema version, it
preserves both files byte-for-byte, does not save over them, opens an in-memory
fallback, and shows an upgrade-required notice.

Unknown surface types, missing resources, and temporarily unavailable runtimes
remain in place as recoverable placeholder tabs. Placeholders show the persisted
identity and allow Retry, Locate/Rebind where meaningful, Close, and Reset
Surface. Restore never silently drops a user's tab.

Recovery actions are scoped:

- Reset Surface resets one surface's registered state.
- Close Group follows the deterministic, all-or-nothing close behavior above.
- Reset Workbench replaces layout/presentation state with the default.

Reset Surface and Reset Workbench run the same registry close/dirty guards as
tab closure. Reset Workbench evaluates groups in tree depth-first order and tabs
in visual order, offers Save/Discard/Cancel for dirty state, awaits successful
saves, and commits one replacement transaction only if every guard resolves.
Any cancellation or failed save leaves the entire workbench and persisted
revision unchanged.
An accepted Reset Surface also detaches persisted duplicate-presentation
provenance from both endpoints in the same guarded compare-and-swap transaction.

None of these actions stop agents, kill PTYs, delete workspaces, or clear domain
data.

### Versioning and first migration

The persistence module owns sequential, tested migrations. The current
`viewMode` is unpersisted React state and therefore cannot be recovered across a
restart. Version 1 defaults existing users to one Agents Overview surface. While
the feature-flagged old shell and workbench coexist in one running process, the
currently selected view may seed the first workbench once, but it is never
presented as historical persisted state.

Version 1 performs a one-time, validated import of shell dimensions/open state
from the existing `wardian-layout` Zustand localStorage value, clamps them to
current bounds, writes `workbench.json`, and records completion. Corrupt or
missing localStorage uses defaults. Team/watchlist data and domain stores are
not migrated. After the backend acknowledges the imported document, Wardian
removes the `wardian-layout` localStorage value and removes the corresponding
Zustand persistence middleware. Shell/layout fields then have exactly one
durable writer. Surface-specific settings move to their owning stores instead
of remaining in a shadow layout blob. Later versions must not depend on
layout-library private serialization.

## Layout Renderer Boundary

Dockview is the leading adapter candidate because it supplies pane-local tabs,
splits, drag/drop, and keyboardable group mechanics. It must pass a technical
proof before adoption.

The proof uses React 19 and exercises:

- xterm renderers with one owner and several mirrors;
- Graph/Garden heavy renderers;
- 20 open tabs across four groups;
- tab move, group split, group close, and group zoom;
- keyboard tab/group traversal and ARIA semantics;
- restoration from Wardian's model, not Dockview JSON.

Reject Dockview if Wardian would need to persist its opaque/private schema,
delegate surface lifecycle to component mount behavior, or bypass Wardian's
command/registry model. The adapter interface must make a custom renderer or
another library replaceable without migrating workbench documents.

Wardian makes Dockview's 100 by 100 CSS-pixel group minima explicit. A shared
tri-state admission check uses the live destination geometry for edge previews,
drop commits, pane/tab menus, and keyboard commands: measured destinations below
twice the relevant minimum are blocked, exact-boundary destinations are allowed,
and transiently unmeasured destinations remain available. Center moves do not
create a split and are always admitted. The drop path repeats the check so a
resize between preview and commit cannot persist an impossible 50/50 split.

## Interaction and Accessibility

Required commands include Open Surface, Quick Open, Close Surface, Reopen Closed
Surface, Next/Previous Tab, Focus Next/Previous Group, Move Tab to Group,
Split Right/Down, Toggle Group Zoom, Focus Left/Right Dock, and Reset Workbench.
Defaults follow platform conventions and remain remappable when Wardian's
command system supports keybinding customization.

`Ctrl/Cmd+P` opens the transient Quick Open palette and
`Ctrl/Cmd+Shift+P` opens the transient command palette. Neither is a permanent
toolbar or command bar. Search, arrow-key selection, Enter, Escape, disabled
command state, and focus return are part of the palette contract.

Tab strips use the ARIA tabs pattern with roving focus. Split separators are
keyboard adjustable and expose values to assistive technology. Drag/drop always
has a command/keyboard equivalent. Focus returns predictably after close, move,
restore, and placeholder recovery.

On narrow screens, the visual adapter may present one group at a time and turn
side regions into drawers. It does not rewrite or discard the split tree. Remote
clients may select a presentation but participate in the same ownership lease.

## State Ownership

| State | Owner |
|---|---|
| agent/provider/session lifetime | Rust backend |
| PTY owner, epoch, geometry, terminal stream sequence | Rust terminal broker |
| teams, watchlists, queue, workflows, library domain data | existing domain stores/backend |
| active roster/watchlist filter and global command target selection | shared agent/roster controller |
| Agents Overview focused agent and mode | Agents Overview surface state |
| split tree, groups, tab placement | WorkbenchModel |
| registered presentation state | surface instance + registry serializer |
| xterm viewport, local scale, pan | terminal presentation |
| shell region visibility/sizes | layout store, serialized through workbench boundary |
| drag preview, DOM focus, hover | renderer only, never persisted |

`App.tsx` coordinates these owners; it must not duplicate them.

## Delivery and Cutover

### Phase 0: technical proof and performance baseline

Build the layout-adapter proof with the lockfile-resolved React 19.2.4 and an
isolated seeded `WARDIAN_HOME`. Record tab/split interaction latency, React
commit cost, renderer count, terminal output behavior, production bundle delta,
dependency license, release cadence, and maintenance risk. The existing
performance script must be changed to fail closed when no isolated home is
provided before it is used.

Results and thresholds live in
`docs/research/workbench-navigation/dockview-evaluation.md` with a machine-readable
JSON baseline beside it. The phase ends with an explicit Promote or Reject
decision. The proof is then removed or promoted into the production adapter; no
permanent parallel prototype stays in the app.

### Phase 1: workbench foundation behind a feature flag

Land Wardian-owned types, normalized model, surface registry, commands, Home,
layout adapter, persistence, placeholders, and unit/property tests. The flag is
developer-only and defaults off until restore safety is proven.

### Phase 2: terminal broker under the existing UI

Land authoritative multi-presentation registration, lease epochs, snapshots,
sequenced output, desktop/remote adapters, and native tests before changing the
main navigation.

### Phase 3: migrate current surfaces

Register Agents Overview, Dashboard, Queue, Graph, Garden, Library, Workflows,
and Agent Session. Route roster and auxiliary object actions through the
NavigationService. Remove the fixed titlebar main-view launcher only when all
current destinations are reachable through surfaces and commands.

### Phase 4: splits, duplicates, zoom, and full restoration

Enable production drag/drop, split commands, multiple Agent Session
presentations, explicit lease transfer, group zoom, responsive collapse, and
cross-restart restoration.

### Phase 5: cutover and cleanup

Enable the workbench by default, seed the first workbench as described above,
delete old `viewMode` navigation state, remove compatibility adapters after one
stable release, update user/developer guides, and close or explicitly defer
every epic child.

For that first stable release, retain a `WARDIAN_WORKBENCH_SAFE_MODE` rollback
renderer. It uses the same WorkbenchModel/registry and preserves the persisted
document, but presents one active group without Dockview splits and does not
write a downgraded schema. It is not a second navigation model or writer. Remove
the safe mode only after release telemetry and recovery reports are reviewed.

At no point may two navigation models or two terminal geometry writers be
long-lived canonical state. Compatibility paths are adapters with explicit
removal tasks.

## Testing and Verification

### Unit and property tests

- Workbench model normalization and invariants after every command.
- Random command sequences never lose a surface, duplicate an ID, create an
  empty invalid split, or reference a missing group.
- Registry serialization/restore and placeholder preservation.
- Persistence migrations, atomic write fallback, corrupt primary/backup, and
  unknown versions.
- Terminal lease transfer, stale epochs, owner loss, geometry rejection,
  snapshots, sequence barriers, and gap recovery.
- Agents Overview grid scoring across container sizes and counts.

### React integration tests

- Home discovery, singleton focus, explicit duplicate/open-to-side.
- Tab order, group activation, close guards, zoom, keyboard traversal, and
  accessible roles.
- Left rail remains auxiliary-only; its object actions route via navigation.
- Right roster focuses or opens Agent Session.
- Heavy surface suspend/resume and state restoration.
- Terminal mirror remains read-only until explicit activation.

### Browser E2E

- Launch, open all migrated surfaces, split, move, close, reopen, quick-open,
  reset, and restore mocked workbench state.
- Auto/Grid/Single Agents Overview behavior based on measured containers.
- Responsive one-group presentation preserves the desktop split model.
- Corrupt/missing resources render recoverable placeholders.

Browser E2E does not make PTY or native persistence claims.

### Native E2E

- Desktop owner plus desktop mirror with stable geometry.
- Desktop/remote ownership transfer and return.
- Stale input/resize rejection without disconnection.
- Output snapshot/barrier correctness through transfer.
- App-created workbench state is readable on disk and restores after restart.
- Closing all agent presentations leaves the runtime alive.

Provider-specific tests run only when a behavior depends on a real provider.

### Performance gates

Before default cutover, capture repeatable baselines for:

- startup and restore with 20 tabs / four groups;
- tab switch and group focus latency;
- continuous terminal output with one owner and three mirrors;
- Agents Overview layout changes under rapid container resize;
- Graph/Garden suspension and resume memory/commit cost.

Targets are established from Phase 0 on supported CI/development hardware and
recorded in the implementation plan. Regressions must be visible; unmeasured
claims such as “instant” are not acceptance criteria.

### Required repository verification

- `npm run lint`
- `npm run test`
- `npm run build`
- `npm run docs:build`
- `npm run test:e2e`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace -- --test-threads=1`
- `cargo check --workspace`
- targeted native E2E for terminal and persistence claims
- feature-specific screenshots under `e2e/screenshots/workbench-navigation/`
  with at least one representative HTTPS image embedded in the PR body

The cutover task audits and migrates every source, unit test, browser/native E2E,
documentation capture script, and performance script that selects the old
titlebar tabs. Adding new workbench tests without removing old-selector coupling
is not sufficient.

## Documentation and Issue Governance

- Epic #513 is retitled/rebased around Workbench Navigation and points to this
  spec.
- Existing children #514-#523 are rewritten, superseded, or closed according to
  the phases above. No Site/Cohort implementation is retained merely to satisfy
  old issue bookkeeping.
- `docs/guide/` documents surface opening, tabs/splits, Agents Overview modes,
  restore/reset, and terminal activation.
- `docs/developer/` documents registry contributions, persistence migrations,
  layout adapter boundaries, and terminal broker invariants.

## Consequences

- **Positive:** New capabilities become registered work objects instead of new
  permanent titlebar modes.
- **Positive:** Multi-agent monitoring and focused work share one model without
  conflating terminal presentation with runtime lifecycle.
- **Positive:** The shell remains recognizable and the left rail retains its
  coherent auxiliary-tool role.
- **Positive:** Versioned Wardian-owned persistence remains inspectable and
  replaceable independently of the layout library.
- **Positive:** Desktop and remote terminals gain one explicit, testable
  geometry/input ownership contract.
- **Negative:** This is a multi-phase architectural migration spanning React,
  Tauri, xterm, persistence, browser E2E, and native E2E.
- **Negative:** Separate renderer instances increase frontend memory usage; the
  lifecycle policies and performance gates are necessary to control it.
- **Negative:** A third-party layout adapter still carries integration risk,
  which is why Phase 0 has an explicit rejection gate.

## Rejected Alternatives

### Keep fixed global views and add tabs inside some pages

This creates two competing navigation models and cannot place unrelated
surfaces side by side.

### Move main-view selection to the left rail

The rail already selects contextual tools. Turning it into a mixed page/tool
launcher obscures the shell boundary and conflicts with the established design.

### Make Sites and Cohorts foundational now

The terms add hierarchy without a validated user need and improperly couple
layout to agent organization. Named workbench contexts can be designed later
from observed workflows.

### Let every terminal presentation resize the PTY

This makes geometry nondeterministic and lets remote or small split clients
break desktop rendering.

### Use smallest-client-wins terminal geometry

It stabilizes shared text visibility by degrading every client to the smallest
viewport. Wardian instead preserves one explicit owner's canonical grid and
adapts mirrors locally.

### Persist the layout library's native JSON

It would leak a replaceable dependency into Wardian's durable state and make
surface lifecycle/recovery dependent on library internals.
