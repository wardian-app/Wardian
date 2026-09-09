# Garden material styling and variable collections

- **Date:** 2026-09-09
- **Status:** Implemented; local validation and independent review passed
- **Precedence:** Extends the September 8 object-unfolding and continuous-zoom specs. This is a visual revision; the shared camera remains authoritative.

## Experience contract

Garden helps operators orient among agents, workspaces and recorded evidence. The Makepad reference is primarily a styling reference: tangible colored surfaces, defined boundaries, shallow relief and coherent contained objects. The user specifically corrected an interpretation focused on unfolding mechanics, which already exist. They endorsed bounded labels and activity overlays, and required composition to accommodate dozens of conversations and hundreds of memories per agent.

Two directions were compared: colored inset trays and packed specimen chambers. Inset trays were selected because they give sparse collections intentional space while retaining stable geography for dense collections. Packed chambers would require a new aggregate-object navigation model. This revision retains the existing five organic regions, shared camera, source record anchors and canonical actions.

Visual references: [Makepad](https://github.com/makepad/makepad) and the
[code landscape demonstration](https://x.com/rikarends/status/2097207150850273640/video/1).
The reference informs material and composition; it does not establish Wardian's
performance or require a replacement renderer.

## Material system

Use three levels: membrane, organelle tray, individual object. Jade, ochre, slate and heather are theme-aware material roles, separate from runtime status colors. Thin rims, upper highlights and lower edges supply shallow relief. Skills retain their marks; memory seeds retain their source outline; conversations use folded slips; files and directories use nested leaf/folder marks. Workspace activity evidence remains attributable or inferred according to its existing canonical source.

Labels must fit within their surfaces. Compact captions use ellipsis, with full names in accessible descriptions and hover titles. Reading planes retain full text and evidence. Empty space is intentional; sparse regions must not invent filler objects or statistics. Regions do not resize with count and displace their neighbours.

## Variable collections

Memory retains kind/scope compartments, truthful loaded counts, and individual anchors. Above 48 loaded records, Find memory searches canonical text and moves focus to the next match in the retained grid. Finding does not filter, reorder or remove source objects. The camera and an already selected record anchor remain unchanged.

All loaded conversation summaries are reachable under Sessions & Inbox; the former silent three-entry cap is removed. Entries show date, status, excerpt and recorded counts. Expanded excerpt bodies mount only when opened. These are archive summaries, not a promise of complete transcripts or a new conversation editor.

The retained collection is deliberately not virtualized in this revision: scroll and selection must preserve record anchors. Testing 300 memories and 60 conversations establishes bounded scenario evidence only; it does not establish arbitrary-size performance or native-provider throughput.

Persistent zoom and Fit controls sit above world cells so a membrane extending
beyond a resized viewport cannot intercept the recovery controls. Backward
memory search leaves clearance below the sticky search field; memory scope
counts use primary text contrast in both themes.

## Validation

Inspect sparse and 300-memory / 60-conversation agents in light and dark themes. Find the final memory, enter its canonical reader, return, and open the oldest conversation. Verify unchanged world geometry, ordinary reversible wheel zoom, keyboard access, narrow reading planes, reduced motion, stale/loading/error notices and canonical exits. Compare representative production RAF scheduling and long tasks with the same fixture, separating those measurements from actual presented FPS. Refresh the guide image and publish a zoom video and focused screenshot evidence in the issue-linked PR.

### Recorded evidence

The integrated frontend verifier passed 3,650 tests with one skip across 273
files, plus build, typecheck, lint, all seven debt-budget groups and the remaining
frontend checks. Documentation verification passed. The final camera-control
layer fix additionally passed build, scoped lint and the real resize/Fit browser
regression. Independent local source and integration review ended at zero
blocking findings after closing backward-search clearance, count contrast and
camera-control layering issues.

A production probe on `00d5177c` at 1600 × 1000 used 61 agents, 43 workspaces,
46 routines, 300 memories and 60 conversations. Around an already-loaded agent,
wheel-in/out, finding memory 299 and opening conversation 60 had RAF p95 values
of 16.8/16.7/16.7/16.8 ms respectively, with zero observed long tasks in those
sampled actions. This is one warm cycle and scheduling evidence, not presented
FPS. Habitat entry, full record travel, active providers and native rendering
are outside this probe's scope. The separate 43-second production walkthrough
shows the complete hierarchy and canonical record return.

The complete supported DEV browser suite passed on an isolated `00d5177c` checkout: 194 passed, 19 existing skips, zero failures or retries (5.3 minutes). The Garden journeys also passed against the frozen production build. Development-only dynamic store imports and the Workbench proof hook require the DEV harness; they are not production application entry points.

