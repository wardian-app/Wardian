# Garden Composition Implementation Contract

- **Status:** Historical implementation; navigation and visual acceptance superseded
- **Date:** 2026-09-07
- **Design:** [Garden Semantic Zoom Composition](./2026-08-30-garden-semantic-zoom-composition.md)
- **User documentation:** [Garden](../guide/garden.md)

The [September 8 continuous-zoom contract](./2026-09-08-garden-continuous-zoom.md)
supersedes this document wherever navigation, camera ownership, presentation,
content scrolling, or visual acceptance differ. The user rejected the abrupt
switch from the world into a text-heavy DOM cutaway. Continuous camera travel
into world-anchored cells and records is fundamental intent, not optional polish.
Integration and validation of the replacement are pending.

The sections below preserve the September 7 implementation record. Its recorded
checks remain evidence for that historical implementation only; they do not
establish acceptance or passing tests for the replacement. Canonical data,
attribution, and authority rules remain applicable unless explicitly revised.

## Historical Hierarchy and Navigation (Superseded)

Habitat exposes stable districts and agent signals; workstream scale reveals
named cell-like inhabitants, workspace ground, skill marks, and situated
routines. Agent, workspace, and automation compositions branch into readable
records. Focus is a selection/entry transition, not a new population layer.
Interiors use a themed DOM reading container over the retained canvas; this
slice does not require a continuous same-object vector morph.

Single click or Space selects without moving the camera or leaving Garden.
Double-click or Enter enters the next composition/record. Selection supplies an
explicit entry button for touch. Escape and breadcrumbs return through the
trail, restoring the departed frame's camera; port jumps append a reversible
frame even when revisiting an earlier object.

District density uses screen diameter with hysteresis: enter workstream at
340 px and retain it down to 280 px. Selected-object zoom can enter at 280 px
for agents/routes, 340 px for districts, and 400 px for workspace/path extent,
with a 40 px re-entry dead band. Empty-space zoom only changes the camera.
Composition return is explicit through Escape/breadcrumbs; reverse wheel
morphing and richer focus previews from the design are not acceptance claims.
Agents are draggable only at workstream scale and remain clamped to their
canonical district. Derived ground, routes, and interior regions are not draggable.
The retained canvas keeps a drawable backing surface during zero-size pane
transitions; fit and viewport reporting still use real measured dimensions.

## Canonical Actions and Authority

GardenView owns selection, the navigation trail, and the composition boundary.
Interior components render content and request selection/entry/actions from
that owner. A projection never establishes exclusive ownership or grants access.

| Record or action | Authoritative owner and behavior |
| --- | --- |
| Agent Identity | Agent configuration and backend session lifecycle; saved permissions/tools are configuration, not proof of runtime application |
| **Open agent session** | Contextual Workbench navigation from the Garden source surface to `agent-session` with the agent session ID; terminal presentation, bypassing Agents overview reuse |
| Skill / **Open in Library** | Library definition and deployment provenance; linked versus copied, direct/class/global scope |
| Memory | Canonical memory list/get/history reads; Stable/Current, agent-wide/workspace-bound, evidence, sources, and revisions; no separate Garden memory editor |
| File / **Open file** | Canonical file resource reads; changed text opens the comparison with Garden's baseline, other files follow file-open preferences |
| Routine / stage | Canonical blueprint, schedule, invocation, checkpoint, and event records; **Open automation definition** delegates to file opening |
| Active work | Conversation records and Inbox store; Inbox attribution uses stable `agent_session_id`, never names or guessed blueprint ownership |

Garden's session action is intentionally different from Graph/Inbox, which
retain existing Agents-view reuse. Cross-surface navigation preserves Garden;
Workbench Back or closing the destination returns to the originating context.
`navigationService` tracks the Garden opener when opening or reusing a canonical
destination, including contextual session retargeting. An allowed close of the
active destination focuses that opener if it still exists, preserving Garden's
surface state even when another tab is adjacent. Closing a background destination
does not steal focus, and a cancelled close does not perform the return. This
opener association is held in the navigation service's runtime map; it is distinct
from the persisted Garden trail and camera, not a promise of return tracking
across an application restart.
Opening a changed-file comparison uses the existing review-watermark path for
its attributed agents. Selection and reading the Garden file record alone do
not mark the comparison reviewed. Schedule editing and approval decisions stay
with their existing surfaces.

## Activity and Situated Execution

Map terrain requests immediate workspace-root listings (`rootOnly`); it does
not recursively fetch a filesystem tree as the default activity view. Workspace
composition derives deeper groups from changed-path ancestry, including deleted
leaves. **Show full tree** explicitly requests paged directory contents.
Selecting an agent highlights attributed paths; selecting a path/group reveals
participating agents. Evidence labels distinguish attributed and inferred
writes; absent attribution must not invent an owner.

**Now** means newest two turns; **Recent**, the default, means newest sixteen.
The current known-recency predicate includes turn distances up to 2 or 16 from
the change summary's newest turn, using its eight-turn half-life paint value.
Unknown/inferred recency is retained rather than silently removed. Paint carries
an explicit `recencyKnown` flag: any descendant with unknown turn recency makes
the ancestor's recency uncertain, even when other descendants are attributed
and dated. The lens retains `recencyKnown === false`, including attributed files
without usable turn timestamps. **Branch**
removes the time cutoff from the current workspace comparison; it does not
select a different baseline. Garden accepts workspace-wide HEAD/branch-point
preferences and falls back to branch point for agent-scoped preferences. Record
evidence names the resolved baseline and available turns.

Automation identity is `schedule:<id>`, `binding:<blueprint-id>`, or
`run:<run-id>`, not a union keyed only by blueprint. A direct agent reference
can situate a binding without a schedule; unresolved role/class requirements
cannot. Scheduled runs enrich their schedule, while unscheduled runs retain
individual identity. One assigned agent produces an attachment, several a
directed route, and zero agents a workspace routine when workspace evidence
exists. Without an anchor, or with missing map participants, no location is
fabricated.

Running/awaiting-approval runs remain eligible regardless of age. Other runs
use a separate **24-hour** window, testing update time, then completion or
start time when absent. This is independent of the file lens. Schedule
projections survive quiet periods; historical runs remain in Automation Monitor.
Open automation/stage compositions also retain their evidence beyond this
window. `useGardenAutomations` accepts the automation IDs in Garden's trail and
returns their evidence separately as `retainedAutomations`. GardenView combines
that evidence with current projections only for composition lookup and labels;
it never feeds retained-only evidence into map placement or canvas routes.
Retention follows the trail, including its focused automation and parent of an
open stage; returning beyond that trail entry releases the retention request.
An open record therefore does not expire merely because its map trail ages out,
and inspecting history does not repopulate the live map.

Stage order derives from blueprint execution order and collapses consecutive
same-agent owners while preserving a return such as A → B → A. Concurrent runs
have separate lanes ordered by start time, using their own available immutable
blueprint snapshot, invocation assignments, stage state, and event evidence.
Live evidence outranks saved schedule previews. Workspace anchors use normalized
path identity and the workspace's stable local anchor when available.

Stage-local failure and awaiting-approval marks are implemented on the canvas,
derived from node state and ordered run events at the actual agent or workspace
assignment. Marks appear above agent bodies with text and ×/! cues; route hit
targets remain below agents. Temporary providers with live/recent run evidence
use labelled dashed silhouettes at their workspace anchor, without becoming
durable agents. Dormant assignments alone do not create temporary inhabitants.
Unlocatable attention is counted in the route summary rather than assigned an
invented position. Route styling distinguishes quiet, paused, and live execution;
summaries include concurrent active-run counts and stage labels are also exposed
through the semantic-object controls.

## Persistence, Access, and Failure States

Each Garden Workbench surface persists `selected_unit_key`, `trail`, `camera`,
and `time_lens`; trail frames retain return cameras. Restoration validates
reference kinds, finite coordinates, and finite positive camera scale, tolerating older
selection-only state. The existing `wardian-garden` browser scene store owns
district cells, visits, manual district-relative pins, and settled geometry.
It is not yet an inspectable scene file under `<wardian-home>`. Workbench
persistence owns the surface state; neither store duplicates canonical records.

Memory/conversation reads load lazily and refresh independently in fixed agent
regions. Stale snapshots are labelled, and switching agents must not flash the
previous agent's contents. **Refresh contents**, record **Retry**, directory
errors, activity-source errors, and automation incomplete/stale notices expose
read limitations. Automation refresh errors are tracked per projection. When
that projection has a known snapshot, it is retained with `stale` and
`evidenceErrors`, while unrelated projections advance from successful reads.
Without a prior snapshot, retention does not invent evidence. The map's expiry
rule still applies to stale run projections; only trail-requested composition
evidence can remain past the activity window. File records use the existing file-resource access boundary;
denied or unavailable reads are errors, not permission grants or empty success.
Removed agents, unavailable routine evidence, and invalid saved stage references
retain breadcrumb recovery. Paging controls disclose bounded catalogs.

The canvas supplies labelled semantic-object controls with roving focus,
Space/Enter/Escape, and arrow/Home/End traversal; canvas-focused arrows pan.
In the superseded interaction model, DOM interiors own focus while open, retain named regions and explicit actions,
and report loading/errors through status/alert text. Reduced motion disables
the composition entrance animation. Focused browser checks cover keyboard,
narrow-layout, and reduced-motion behavior; they are not a complete assistive-
technology or physical-touch-device audit.

## Historical Verification Boundary

Recorded verification on September 7, 2026, before the continuous-zoom revision
(not current acceptance evidence):

- Full `npm run verify:ci -- --only frontend` passed: typecheck, lint, unit
  tests, production build, Workbench cutover, test reachability, dead code,
  debt budgets, and page fixtures. After the final source-recovery changes,
  the full unit rerun passed **268 files / 3,575 tests**, with one existing skip;
  focused loader tests, lint, and typecheck also passed.
  A subsequent focused canvas rerun passed 19 tests, including zero-size
  mount, each collapsed axis, hide, and restore without fabricating a viewport.
- Focused Garden browser suite: **17 passed**, no retries. It covers selection
  without camera movement, all five regions, memory evidence, activity/full-tree
  browsing, immutable stage outputs, keyboard entry, narrow layouts, reduced
  motion, nested-directory entry by pointer/keyboard/summary, and canvas-margin
  isolation. Canonical session, Observe, and Monitor actions return to Garden
  when their destination closes.
- Full browser suite after the zero-size canvas correction: **187 passed,
  18 existing skips**, no failures or retries. The previously failing
  Workbench adapter scenario also passed its focused rerun.
- Unit regressions cover unknown attributed recency retained in **Now**,
  concurrent assignment lanes, expired focused evidence, independent source
  failure/recovery, denied file reads, and inactive closes not stealing focus.
- Desktop and narrow screenshots were inspected for text clipping, complete
  Ports content, stable regions, and readable labels. The owning guide at that time
  included refreshed reader-facing screenshots; PR evidence uses synthetic data.

Browser fixtures do not prove native file access, PTY behavior, or real provider
execution. Backend and hosted-check outcomes belong in the delivery evidence,
not an inferred claim from frontend success. The implementation adds no new
native command or provider lifecycle. Release notes follow the feature's
Conventional Commit rather than a manually edited changelog.

The stage-local attention marks and temporary-provider silhouettes were
implemented scope. Historically, continuous same-object vector morphing and
directional route animation were deliberately omitted in favor of stable
geography, explicit entry, and legible execution state. The continuous-zoom
omission and explicit-only return are now superseded; the new contract requires
continuous, reversible travel. This correction does not establish a new
requirement for directional route animation.
