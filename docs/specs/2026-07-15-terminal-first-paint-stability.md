# Terminal first-paint stability

## Status

Approved design for the navigation workbench terminal stabilization follow-up.

## Problem

Entering the Agents surface currently reconstructs many terminal renderers at
once. The first visible frame is often drawn with stale terminal dimensions,
then visibly reflows several times before settling.

This is caused by lifecycle ordering rather than provider output:

- hiding Agents clears its resident-agent set and suspends every terminal;
- suspension immediately retires each xterm renderer, bypassing the documented
  short reuse grace;
- returning to Agents recreates the renderers and seeds terminal state before
  final card geometry and renderer backend metrics are settled;
- independent correction paths then fit or refresh on initial attach, reveal,
  WebGL promotion, the next animation frame, fixed 50 ms and 300 ms timers,
  font readiness, and ResizeObserver delivery;
- the terminal host becomes visible before those correction paths finish.

The result is exactly the observed poor render, refresh, and eventual correct
render. More delayed refreshes would increase the race rather than solve it.

## Invariant

A terminal presentation must produce one coherent first visible frame.

Before its host becomes visible, Wardian must know the final measured host
bounds, choose the renderer backend available for that presentation, fit xterm
to those bounds, finish any required snapshot or resynchronization write, and
confirm that none of those inputs changed during preparation.

Later fitting is allowed only after a genuine geometry, font, or renderer-
backend change. A timer expiring is not evidence of such a change.

## Design

### 1. Separate visibility from renderer residency

Agents keeps its bounded resident xterm set when its surface becomes hidden.
The presentations report `visibility: hidden`, but a resident renderer remains
`mounted` while it is within the existing global xterm budget. Hidden Agents
terminals continue receiving the ordered broker stream, so returning to the
surface does not require re-registration, snapshot replay, or renderer seeding.

WebGL residency remains independent. A hidden presentation may relinquish its
WebGL context and continue as a mounted DOM renderer. The existing renderer
budget may still evict an xterm; eviction marks the presentation suspended and
the next reveal reconstructs it through the same preparation transaction used
for a genuinely new renderer.

The resident set remains capped by `MAX_XTERM_RENDERERS`. Hiding a surface does
not add residents or bypass LRU eviction.

### 2. Use one reveal preparation transaction

`AgentTerminal` owns a per-renderer reveal generation. Each new, restored, or
newly visible renderer enters a preparing state and its host stays hidden.

Preparation performs these ordered steps:

1. Read the connected host's current non-zero bounds after the Agents layout
   commit.
2. Select or restore the renderer backend. WebGL promotion is attempted only
   when physical intersection and the budget allow it; DOM is a complete,
   valid fallback rather than an intermediate frame.
3. Fit xterm once from the current host bounds and current cell metrics.
4. For a new or evicted renderer, apply and await the broker snapshot or owner
   resynchronization against that fitted renderer. A preserved resident skips
   this step because it remained subscribed.
5. Re-read bounds and cell metrics. If they changed during preparation, restart
   the transaction under a new reveal generation while the host is still
   hidden. Otherwise remove any snapshot overlay and reveal the host.

Only the latest reveal generation may set the renderer ready. Hiding,
suspending, eviction, replacement, or unmount invalidates an in-flight
generation so stale asynchronous work cannot reveal or refresh a replacement.

### 3. Remove correction-by-timer behavior

The initial next-frame, 50 ms, and 300 ms fits are removed. The current
fit/refresh/fit reveal sequence becomes the preparation transaction above.
WebGL activation no longer performs a visible post-reveal correction.

Font readiness participates as an input to preparation when a font load is
already pending. A later real font-setting or loaded-metric change schedules
one new generation rather than an unconditional refresh.

ResizeObserver remains active after reveal, but it coalesces one fit per frame
and compares measured bounds and proposed rows/columns with the last committed
geometry. Unchanged observations produce no xterm resize, refresh, broker
viewport report, or PTY resize. A changed observation performs one fit and one
owner-aware geometry report.

### 4. Preserve broker authority

This change does not transfer terminal ownership. Hidden presentations remain
non-input-capable even when their renderer is resident. Only the active lease
holder may resize the native PTY; mirrors fit locally and report their desired
viewport without changing canonical geometry.

Renderer preservation never changes runtime generation, lease epoch, stream
sequence, or presentation identity. Clear and runtime replacement still force
the existing generation-aware snapshot/resynchronization path.

## Failure handling

- Zero-sized or disconnected hosts remain hidden and wait for a measured
  layout observation.
- WebGL failure commits the DOM backend and continues preparation; it does not
  retry repeatedly during the same reveal.
- Snapshot or resynchronization failure keeps the renderer hidden and exposes
  the existing retryable renderer error state.
- A renderer-budget eviction invalidates preparation before retiring xterm.
- A provider that emits output during preparation is safe: ordered writes
  finish before the reveal barrier, and only the current renderer generation
  may be mutated.

## Verification

### Frontend unit tests

- Hiding and quickly showing Agents preserves resident xterm instances and
  does not request another snapshot.
- Hidden residents remain broker-hidden and cannot submit input.
- A fresh renderer stays visually hidden until fit and snapshot writes finish.
- First reveal commits exactly one terminal grid size and performs no fixed-
  delay fits.
- WebGL success and failure both reveal one coherent backend frame.
- Stale reveal generations cannot reveal, refresh, or resize replacements.
- Repeated unchanged ResizeObserver deliveries do nothing.
- Genuine size and font-metric changes perform one coalesced refit.

### Browser E2E

Browser tests verify Agents residency and DOM visibility ordering with mocked
terminal infrastructure, but do not claim native PTY correctness.

### Native E2E

A native debug test switches repeatedly between Agents and another surface
while several live mock-provider terminals are present. It records per-
presentation renderer identity, ready transitions, fit count, resize count,
grid geometry, and visible host bounds. Each return to Agents must:

- reuse every non-evicted renderer;
- expose no visible frame whose grid differs from the final grid;
- issue no snapshot request for a preserved resident;
- produce at most one committed fit when geometry is unchanged;
- preserve terminal content, input, owner identity, and scroll position.

The same test covers an intentionally evicted renderer and proves that its
reconstruction remains hidden until the final geometry is ready.

## Rejected alternatives

Recreating every renderer behind a longer hidden staging phase still wastes
snapshot and WebGL work on every tab switch and makes return latency scale with
the number of agents.

Forcing Agents terminals to the DOM renderer would reduce context churn but
would sidestep lifecycle ordering and create visual differences between Agents
and Agent Session surfaces. DOM remains the supported fallback, not a separate
Agents rendering policy.
