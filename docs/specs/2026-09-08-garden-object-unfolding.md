# Garden object unfolding and bounded rendering

- **Date:** 2026-09-08
- **Status:** Implemented; frontend and browser verification passed; final density/native evidence below
- **Precedence:** Extends [continuous zoom](./2026-09-08-garden-continuous-zoom.md); supersedes its text-heavy interior presentation and unbounded retained drawing.

## Intent

The Garden is a spatial lens over canonical agent, memory, skill, workspace and
execution records. The same objects should become more understandable as the
camera approaches them. The user accepted the continuous camera direction but
found the interiors too text-heavy and asked for the session-size performance
check and unfolding research to be implemented.

The inspected references were [OneZoom](https://www.onezoom.org/),
[Scale of the Universe 2](https://htwins.net/scale2/),
[Zoomquilt](https://zoomquilt.org/) and [Arkadia](https://arkadia.xyz/).
The transferable sequence is outer shape, contained objects, short labels,
then a readable record. These references are design inspiration, not evidence
that Wardian can support their populations or rendering performance.

## Objects before prose

The five organelles keep their geography and the shared camera remains fully
reversible. Skills use their existing marks; memories use individual seeds
grouped by Stable/Current and canonical scope; routines show a bounded preview
of execution nodes with recorded status colors; Ports use connection endpoints.
Short labels fade in without changing object positions. Focus, hover and
selection reveal a label completely. Accessible names retain canonical titles
and provenance even when the visual preview is compact.

Memory captions are outside the seed's measured source plane. Approaching a
seed expands the reading plane about that seed, without a tall caption causing
the plane to shrink during inward travel. Full memory text, revision history,
evidence and scope remain in the canonical record reader. Conversation and Inbox
evidence is available through the Sessions & Inbox disclosure. Configuration
and provenance remain inspectable; the Garden does not become another editor.

Run compositions retain separate execution lanes. Stage nodes carry ordinal,
name and status; assignment detail arrives on approach. Enter continues to the
same immutable stage evidence and canonical Observe/Monitor actions.

## Bound work without losing continuity

Canvas drawing must exclude offscreen objects and work whose complete visual
contribution has faded away. Culling includes visual footprints and drag
exceptions; it must not alter layout inputs, saved positions, keyboard entry,
pointer anchoring or the smooth transition to DOM contents. Overview routes
remain recognizable without every routine name competing for the same space.
Any buffer optimization needs controlled profiling and visual verification.

Agent canonical reads start near inspection scale (360 projected pixels) and
stop below 280 pixels. A view-owned bounded cache retains successful memory and
conversation snapshots for 30 seconds, shares pending reads and preserves stale
snapshots after refresh failure. Explicit refresh bypasses freshness. Unmounted
views do not leave a module-global cache of private contents. Record readers
also defer until approach, using the same 360/280 hysteresis.

Automation summaries still refresh every 15 seconds. Successful blueprints are
cached for at most 60 seconds; completed run evidence is keyed by its complete
summary and cached for at most five minutes. Live run evidence stays fresh.
Library changes and explicit refresh invalidate the cache; expiry catches
same-path edits without an event. Failed or cancelled reads cannot publish a
fresh cache entry. Cache maps are bounded and pruned to the retained population.

Camera persistence coalesces after 250 milliseconds of quiet. Selection and
navigation persist immediately; unmount flushes the latest camera. This reduces
Workbench writes while preserving the final viewing position.

## Acceptance evidence

### Screenshot-driven consistency correction

At habitat overview scale, ordinary automation routes and their anchors recede
continuously instead of forming an always-visible web. Selected routes remain
visible, and local attention markers remain available. Hidden routes have no
pointer hit area. Connections return smoothly as the camera approaches.

Map coverage is an explicit, initially collapsed disclosure beside navigation.
It distinguishes additional automation definitions from additional run records,
and explains that checking more records may leave the map unchanged. The run
catalog is ordered by update time, so later records can still contain active or
recent executions; incomplete coverage must not be presented as complete.
Folder coverage uses a separately named action in the same disclosure. Routine
pagination is no longer a warning floating over every level of the canvas.

Organelle headings and counts share a centered alignment, with symmetric scroll
gutters and safe insets from curved boundaries. The session action may wrap
inside the nucleus. A selected source seed and its outline fade as the record
membrane unfolds; the memory membrane retains an asymmetric seed shape before
settling into a lightly tinted reading plane. Its coarse caption disappears
before record prose appears. Memory records now anchor to the visible seed
rather than its padded button; that captured center and width stay fixed during
reversible camera travel. Padding and caption changes cannot reshape the source.

### Validation protocol

Use the same synthetic 61-agent, 43-workspace, 46-routine population that exposed
the original stalls, including 2437 stored historical runs and the normal
200-summary page. Measure RAF intervals, long tasks and command counts in a
production browser build. Include dense interiors and a 200-agent stress case.
RAF intervals are scheduling evidence, not compositor-presented FPS.

Verify actual agent-to-memory-to-record travel and reverse zoom, selection,
keyboard reading, placement dragging, narrow viewports, reduced motion,
loading/error/stale states and canonical exits. Record a zoom video from the
current implementation and publish sanitized evidence with the PR. Native
WebView and real IPC results must be identified separately from browser mocks.

### Verified interaction and source behavior

The frontend verifier passed all checks, including 3,621 tests with one existing
skip across 271 files. The full browser suite on `dadf17e2` passed 192 tests with
18 existing skips and zero retries. The 16 Garden tests include dense 34-memory scrolling,
unchanged pointer anchoring and reversible growth, canonical exits, placement
dragging, narrow reading and deferred canonical reads. A separate production
build exercised the Garden tests as well. Reader-facing screenshots were
refreshed from this implementation.

Independent local-agent source review ended at zero blocking findings after
preserving complete memory text in accessible names and tooltips, while keeping
visual captions compact. Wrapped stage connectors are a non-blocking follow-up
in [#1229](https://github.com/wardian-app/Wardian/issues/1229).

### Measured rendering and refresh cost

Production Chromium measurements on `dadf17e2` used synthetic 61-agent, 43-workspace,
46-routine fixtures at 1600 × 1000. The dense variant contained 610 skill
deployments and 34 memories per inspected agent. In one complete cycle, dense
wheel-in/out RAF p95 fell from 216.7 ms to 49.9/50.0 ms after the final skill
disk buffer fix; observed long tasks fell from 33/28 to 1/0. Representative
wheel-in/pan/out p95 was 16.8/16.8/33.3 ms. Dense wheel-in still included a
100 ms frame, and district entry reached 116.7 ms: uniform 60 FPS is not
established. RAF measures scheduling rather than presented frames.

Warm automation refresh with eight live executions used 19 commands, retaining
fresh live evidence without reparsing all 46 blueprints. Dense incoming-update
and idle-refresh p95 were 16.7 and 16.7 ms. These synthetic metadata updates do
not represent active PTY streams. A 200-agent run before the final leaf-only
skill fix completed with gesture p95 of 16.8/16.7/33.3 ms; district entry p95
was 83.3 ms. It was not repeated on the final snapshot.

Each case is a single completed cycle, not a replicated benchmark. Browser
fixtures use immediate mock IPC; heap observations include instrumentation
retention and do not establish freedom from leaks. Native population evidence
must remain separate from dense browser interiors and real-provider throughput.

The `dadf17e2` debug Tauri build also completed a real-IPC population probe with 61
synthetic off agents across 43 isolated workspaces, entering an eight-agent
district. Wheel-in/pan/out RAF p95 was 16.8 ms in each phase, with a maximum of
17 ms and no observed long tasks during those gestures. Reverse zoom restored
the initial scale and target rectangle. Source and asset hashes matched the
verified build, including 14 JavaScript assets loaded by WebView. Its canvas was
1072 × 964 with sidebars open. This native probe did not include dense skills,
memory records, automation executions, active PTYs or real providers; it is
population and IPC evidence, not native validation of every dense browser case.
