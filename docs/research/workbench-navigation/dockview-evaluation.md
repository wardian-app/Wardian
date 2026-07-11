# Dockview 7.0.2 adapter evaluation

Decision: Promote (confirmed by the production workbench rebaseline)

Promote `dockview-react` 7.0.2 as the replaceable renderer for the production
workbench adapter. The Phase 0 proof and the Task 18 production-workbench
profile both pass their unchanged gates. This promotes the renderer and
measurement contract, not a second application route or a durable Dockview
model.

## Scope and method

The proof used the lockfile-resolved React and React DOM 19.2.4 on Windows x64
with Node 24.13.1 and Playwright Chromium 147.0.7727.15. The machine-readable
run is in [dockview-baseline.json](./dockview-baseline.json).

The scenario rendered 20 tabs in four groups:

- one writable xterm owner and three independent read-only xterm mirrors;
- real `GraphView` and `GardenView` wrappers with deterministic mock data;
- 14 keyed synthetic renderers;
- model-driven move, split, close, zoom, activation, keyboard traversal, and
  pointer drag paths.

The browser proof mounts the module through a same-origin test document served
by Vite. `App.tsx` is unchanged, there is no `?workbench-proof=1` or other
prototype navigation route, and a production build cannot reach the harness.
Every proof command ran with an explicit OS-temp `WARDIAN_HOME` named
`wardian-workbench-proof-*`. The measurement command canonicalizes the nearest
existing parent before any directory creation, rejects relative paths, the
profile and production home, the workspace root, and symlink or junction
escapes. `--self-test` exercises those guards without running a browser build.

## Production workbench rebaseline

Task 18 replaced the proof-only profile with a built production-workbench
profile. Its machine-readable result is in
[workbench-performance-baseline.json](./workbench-performance-baseline.json).
The command fails before filesystem mutation unless `WARDIAN_HOME` is an
explicit absolute isolated path under the workspace performance root or the OS
temporary directory. Unset, empty, relative, profile-root, production-home,
workspace-root, and canonical path escapes all exit nonzero with the same
refusal message.

The repository source is the deterministic
[workbench-performance-v1.json](../../../scripts/fixtures/workbench-performance-v1.json)
fixture. The profiler stages that fixture inside the isolated home as
`settings/workbench-performance-fixture.json`; this staged relative path is what
the machine-readable baseline records, not a second input. The fixture contains
20 persisted tabs in four groups, 20 mock agents, one terminal owner, and three
mirrors of the same runtime. It includes Agents Overview, Graph, Garden, Queue,
Library, and Workflows. The profiler builds and serves Wardian's flagged
production output rather than the Phase 0 proof route. It measures five fresh
restores, 20 tab activations, four group focus changes, ten ordered terminal
bursts, six Overview resizes, and four heavy-surface resume cycles. React
commits, live xterm renderers, live WebGL contexts, and stream sequence gaps are
instrumented in page. A separate flag-off/flag-on production build comparison
supplies the gzip bundle delta. No observed result is supplied by the fixture or
defaulted when instrumentation is absent.

| Production measure | Observed | Gate | Result |
|---|---:|---:|---|
| Startup restore p95 | 553.52 ms | 1,500 ms | Accept |
| Tab switch p95 | 64.2 ms | 100 ms | Accept |
| Group focus p95 | 31 ms | 75 ms | Accept |
| Terminal output commit p95 | 13.4 ms | 50 ms | Accept |
| Terminal stream gaps | 0 | 0 | Accept |
| Agents Overview settle p95 | 216.16 ms | 300 ms | Accept |
| Graph/Garden resume p95 | 64.3 ms | 500 ms | Accept |
| Maximum React commit | 22.4 ms | 50 ms | Accept |
| Production bundle gzip delta | +90,342 bytes | +256,000 bytes (250 KiB) | Accept |
| Peak live xterm renderers | 13 | 24 | Accept |
| Peak live WebGL contexts | 3 | 12 | Accept |

The first complete production profiling run recorded 15 WebGL contexts against
the limit of 12; every other gate passed. Its counter retained contexts whose
canvases had been detached without emitting `webglcontextlost`, so the value
measured allocation history instead of the concurrent resource peak the gate
is intended to constrain. The correction tracks live contexts by canvas,
removes entries for detached canvases, observes
`webglcontextlost` and `WEBGL_lose_context`, and excludes contexts whose
`isContextLost()` state is true. The 12-context threshold was not changed or
widened. The corrected production run observed a peak of three and passed.

This rebaseline closes the Phase 0 measurement condition: the replaceable
adapter passes in the real workbench composition with production surfaces,
restoration, responsive Overview behavior, and bounded renderer resources.
The final decision remains **Promote**. Native PTY ownership, geometry, and
remote transfer claims remain covered by native tests rather than this browser
profile.

## Candidate facts

| Item | Evidence |
|---|---|
| Package | `dockview-react` 7.0.2, installed exactly rather than through the experimental tag |
| License | MIT |
| Published unpacked size | 3,300,226 bytes |
| React peer range | React and React DOM `^16.8.0 || ^17.0.0 || ^18.0.0 || ^19.0.0` |
| Wardian React version | React and React DOM 19.2.4 |
| Package provenance | npm registry attestation and signed 7.0.2 artifact |

Primary package evidence: [npm 7.0.2 metadata](https://registry.npmjs.org/dockview-react/7.0.2),
[upstream repository](https://github.com/mathuo/dockview), and
[v7.0.2 tag](https://github.com/mathuo/dockview/tree/v7.0.2).

## Adapter boundary result

The proof maintains a separate, plain `DockviewProofModel` with group order,
surface placement, active IDs, and Wardian surface metadata. Restoration calls
public `addGroup` and `addPanel` APIs from that model. It never calls `toJSON`
or `fromJSON`; the source scan recorded zero Dockview serialization references,
and no Dockview JSON is written to disk.

Pointer drops are intercepted as user intent and translated into the same
model command used by the keyboard path. The command computes the Wardian model
transition and then asks the adapter to move the panel. Split, close, activate,
and zoom use the same boundary. Zoom remains renderer-only and does not alter
the serialized model.

Panels use Dockview's public `renderer: "always"` mode. All 20 keyed children
mounted once, and none unmounted during switches, pointer moves, keyboard moves,
split/close, or zoom. Dockview therefore does not need to own Wardian surface or
terminal lifecycle. Replacing Dockview would require a new projection/intent
adapter, not a workbench-document migration.

## Measured baseline

The bundle comparison builds the normal production app and then the same app
with the proof module as an additional production entry. The delta is
conservative: it includes the candidate adapter scaffolding and proof CSS as
well as Dockview.

| Measure | Observed | Phase 1 regression ceiling | Result |
|---|---:|---:|---|
| Production bundle delta, raw | +117,598 bytes | +160,000 bytes | Accept |
| Production bundle delta, gzip | +13,493 bytes | +20,000 bytes | Accept |
| 20-tab/four-group ready | 116.94 ms | 650 ms | Accept |
| Heavy renderers ready | 243.04 ms | 700 ms | Accept |
| Tab switch median / p95 | 33.1 / 38.8 ms | 60 ms p95 | Accept |
| Model command median / p95 | 0.3 / 1.3 ms | 5 ms p95 | Accept |
| React publish-to-layout-effect median / p95 | 1.4 / 7.5 ms | 16.7 ms p95 | Accept |
| Real pointer drag median / p95 | 277.98 / 291.04 ms | 350 ms p95 | Accept |
| 500-line output fanout to four xterms | 16.3 ms | 35 ms | Accept |

Startup uses a fresh browser context after warming Vite's source-transformation
cache, so it does not count one-time dev-server compilation as renderer startup.
The drag number includes Playwright's 16-step human-like pointer movement and
the model convergence wait; it is an end-to-end interaction number rather than
an isolated Dockview handler cost. These ceilings are initial same-machine
regression alarms, not cross-device product promises. CI or supported-device
baselines should replace them when those environments are selected.

The baseline uses schema version 2 and records every ceiling as a
machine-readable `promotion.checks` entry. The command validates non-empty,
finite timing samples, exact fixture and renderer counts, WebGL observations,
surface lifecycle, accessibility counts, and the serialization boundary before
writing the baseline. Missing or invalid evidence and any failed ceiling exit
non-zero instead of emitting a zero-filled or passing result.

## Renderer and terminal observations

- 20 surface renderers stayed mounted, including one Graph wrapper and one
  Garden wrapper.
- Four distinct xterm hosts loaded four WebGL addons with zero load failures.
- The complete page held 18 canvases and seven observed WebGL canvases.
- A 500-line burst wrote 9,500 characters independently to the owner and each
  mirror.
- Moving the owner between groups preserved its keyed child and xterm host;
  Dockview recreated tab chrome but not the Wardian-owned renderer.
- Graph and Garden hid and showed without remounting. The proof therefore did
  not steal a terminal host or delegate heavy-surface state to mount behavior.

This is browser-layer evidence. It does not claim native PTY ownership,
authoritative terminal geometry, or provider behavior; those remain native and
real-provider test responsibilities.

## Accessibility findings

Dockview rendered 20 `role="tab"` elements in four `role="tablist"` elements.
Each tab's `aria-controls` target existed and resolved to one of exactly four
Dockview-owned `role="tabpanel"` elements. Each group had one selected tab, one
roving `tabindex="0"`, and an active panel labelled by that selected tab.
`Ctrl+]` tab traversal worked, and deterministic `F6` traversal moved focus
from group 1's Graph tab to group 2's Garden tab. Pointer movement and the
separate `Alt+Shift+ArrowRight` model command both moved the terminal owner from
group 1 to group 2 without changing renderer identity or mount count.

The Phase 0 gap was split separators: three Dockview sash elements exposed
neither `role="separator"` nor `aria-valuenow`. The production adapter has since
closed that gate with Wardian-owned separator controls that expose orientation,
current/minimum/maximum values and route arrow-key adjustment through canonical
split-ratio commands. React and browser coverage verify that behavior without
making the durable model or command API depend on Dockview DOM selectors.

## Maintenance and release risk

The repository is active and unarchived. npm records stable releases from May
2024 through June 2026, including 6.0.0 on May 2, 2026, 6.6.1 on May 26, and
7.0.2 on June 22. The v7.0.2 npm artifact points to upstream commit
`ecd409e2cb41ae5318610c99b0305b4204ef51af`, and the repository remained active
after that tag.

Maintenance risk is **medium**. Active development, tests, provenance, and
recent fixes are positive. The rapid major/minor cadence, 7.x's recency, one
named npm maintainer, and a large open issue queue mean Wardian should pin exact
versions, keep the adapter narrow, run this baseline on upgrades, and avoid
library-private serialization or lifecycle behavior.

## Promotion conditions

The candidate passed the Phase 0 promotion rules:

- React 19.2.4 rendered without console or page errors.
- Wardian's plain model restored and drove every tested layout command.
- No Dockview serialization or durable schema was used.
- `renderer: "always"` preserved surface and terminal ownership across moves.
- Keyboard tab/group behavior, ARIA tabs, and the production adapter's
  keyboard-adjustable separator controls pass their accessibility coverage.
- The measured bundle and interaction deltas fit the provisional ceilings
  above.

Keep the harness, browser proof, and measurement script only as the contract
for the production adapter slice. Replace or fold them into that adapter when
it lands. Do not add a proof route, persist Dockview JSON, or let this proof
become a parallel navigation model.
