# Dockview 7.0.2 adapter evaluation

Decision: Promote

Promote `dockview-react` 7.0.2 as the leading replaceable renderer for the
production workbench adapter. This is a promotion of the renderer candidate and
the measurement contract, not of a second application route or a durable
Dockview model. Production splits must not ship until the adapter supplies
accessible, keyboard-adjustable separator semantics.

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

The gap is split separators: three Dockview sash elements exposed neither
`role="separator"` nor `aria-valuenow`. This is fixable within the replaceable
renderer adapter by supplying adapter-owned separator controls (or an upstream
public separator hook) that call Wardian split-ratio commands. Production
splits must not be enabled until that path supports arrow-key adjustment,
values, orientation, focus restoration, and React/browser accessibility tests.
The durable model and command API must not depend on Dockview DOM selectors to
solve it.

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
- Keyboard tab/group behavior and ARIA tabs passed; the remaining separator gap
  has a bounded adapter solution and an explicit pre-production gate.
- The measured bundle and interaction deltas fit the provisional ceilings
  above.

Keep the harness, browser proof, and measurement script only as the contract
for the production adapter slice. Replace or fold them into that adapter when
it lands. Do not add a proof route, persist Dockview JSON, or let this proof
become a parallel navigation model.
