# Terminal First-Paint Stability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make entering the Agents surface reveal every resident terminal once, at its final geometry, without renderer reconstruction or correction-by-timer repaint churn.

**Architecture:** Preserve budgeted xterm renderers while their Agents surface is hidden, while keeping broker visibility and input eligibility hidden. Gate every new, restored, or newly visible renderer behind a generation-aware preparation transaction that settles physical intersection, renderer backend, fonts, content writes, and measured geometry before revealing the host. After reveal, only real ResizeObserver or appearance changes may schedule one coalesced fit.

**Tech Stack:** React 19, TypeScript, xterm.js 6, xterm Fit/WebGL/Serialize addons, Tauri IPC terminal presentation broker, Vitest, Playwright, native WebDriver E2E.

## Global Constraints

- The Rust backend remains authoritative for PTY lifecycle, ownership, canonical geometry, runtime generation, and stream ordering.
- The existing limits remain 24 mounted xterm renderers and 12 WebGL contexts.
- Hidden presentations remain unable to submit input even when their xterm renderer is resident.
- DOM rendering is a complete fallback; WebGL failure must not block reveal.
- Only the current renderer and reveal generation may reveal, refresh, resize, or report geometry.
- Fixed 50 ms and 300 ms convergence fits are removed; timer expiry is not a geometry signal.
- Browser E2E must not claim native PTY correctness; native E2E proves presentation reuse and first-paint geometry.
- Preserve the user's existing `package-lock.json` modification and stage files explicitly.

---

## File Structure

- Modify `src/views/AgentsOverviewView.tsx`: keep the bounded resident set across hidden surface intervals and separate renderer residency from presentation visibility.
- Modify `src/views/AgentsOverviewView.test.tsx`: prove hidden resident terminals remain mounted and non-residents remain suspended.
- Modify `src/features/terminal/AgentTerminal.tsx`: implement generation-aware reveal preparation, retain hidden mounted renderers, and consolidate geometry scheduling.
- Modify `src/features/terminal/AgentTerminal.test.tsx`: prove renderer reuse, write/fit/reveal ordering, stale-generation rejection, WebGL fallback, and single-fit ResizeObserver behavior.
- Modify `src/features/terminal/terminalSessionClient.ts`: allow the renderer to complete backend selection and first fit immediately before an initial snapshot is applied.
- Modify `src/features/terminal/terminalSessionClient.test.ts`: prove the pre-snapshot hook is ordered inside the serialized registration transaction.
- Modify `e2e-native/helpers/terminal-debug.mjs`: expose stable renderer identity and reveal diagnostics already owned by the terminal debug snapshot.
- Create `e2e-native/tests/terminal-first-paint-native.test.mjs`: exercise real Tauri/xterm switching with multiple mock-provider terminals.
- Modify `docs/developer/terminal-presentation-broker.md`: document hidden renderer residency and the reveal barrier.
- Modify `docs/guide/workbench.md`: state that short Agents tab switches preserve terminals and reveal only settled frames.

---

### Task 1: Preserve budgeted Agents renderers while the surface is hidden

**Files:**
- Modify: `src/views/AgentsOverviewView.tsx`
- Test: `src/views/AgentsOverviewView.test.tsx`

**Interfaces:**
- Consumes: `MAX_XTERM_RENDERERS`, `surfaceVisibility`, `residentAgentIds`, and existing `AgentTerminalSlot` lifecycle props.
- Produces: separate `isAgentRendererResident` and `isAgentPresentationVisible` decisions; resident hidden terminals receive `visibility="hidden"` and `renderState="mounted"`.

- [ ] **Step 1: Write failing residency tests**

Add tests that render a visible Agents surface, capture its terminal lifecycle props through the existing AgentTerminal mock, hide the surface, and assert:

```ts
const latestTerminalProps = (sessionId: string) =>
  [...terminalRenderSpy.mock.calls]
    .reverse()
    .find(([props]) => props.sessionId === sessionId)?.[0];

expect(latestTerminalProps("agent-a")).toMatchObject({
  visibility: "hidden",
  renderState: "mounted",
});
expect(latestTerminalProps("agent-outside-budget")).toMatchObject({
  visibility: "hidden",
  renderState: "suspended",
});
```

Also show the surface again and assert the same resident IDs are reused rather than cleared and repopulated.

- [ ] **Step 2: Run the tests and verify the lifecycle failure**

Run:

```powershell
npx vitest run src/views/AgentsOverviewView.test.tsx --testNamePattern "preserves resident terminals while hidden"
```

Expected: FAIL because the current hidden surface clears `residentAgentIds` and passes `renderState="suspended"` to every terminal.

- [ ] **Step 3: Separate residency from visibility**

Change the residency layout effect so a hidden surface disconnects viewport observation without clearing the current bounded resident set:

```ts
if (!root || !grid) {
  setResidentAgentIds(new Set());
  return;
}
if (surfaceVisibility !== "visible") {
  return;
}
```

In the card loop, derive and pass independent states:

```ts
const isAgentPresentationVisible =
  surfaceVisibility === "visible" && visibleAgentIds.has(agentId);
const isAgentRendererResident = residentAgentIds.has(agentId);

<AgentTerminalSlot
  visibility={isAgentPresentationVisible ? "visible" : "hidden"}
  renderState={isAgentRendererResident ? "mounted" : "suspended"}
/>
```

Keep the existing `MAX_XTERM_RENDERERS` cap and intersection-driven replacement for visible cards.

- [ ] **Step 4: Run the focused tests**

Run:

```powershell
npx vitest run src/views/AgentsOverviewView.test.tsx
```

Expected: PASS, including the new hidden-residency assertions.

- [ ] **Step 5: Commit the bounded residency change**

```powershell
git add -- src/views/AgentsOverviewView.tsx src/views/AgentsOverviewView.test.tsx
git commit -m "fix(agents): preserve hidden terminal residents"
```

---

### Task 2: Retain mounted xterms without making hidden presentations interactive

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`
- Modify: `src/features/terminal/terminalSessionClient.ts`
- Test: `src/features/terminal/terminalSessionClient.test.ts`

**Interfaces:**
- Consumes: independent `visibility` and `renderState` props from Task 1.
- Produces: a hidden/mounted lifecycle that keeps the current renderer and broker subscription but sets `rendererReady=false`; only suspended render state retires xterm.

- [ ] **Step 1: Write failing hide/show reuse tests**

Add a modern-broker test that renders visible/mounted, waits for ready, records the xterm instance and snapshot request count, rerenders hidden/mounted, then visible/mounted:

```ts
const initialSnapshotRequests = mockInvoke.mock.calls.filter(
  ([command]) => command === "request_terminal_snapshot",
).length;

expect(hiddenHost).toHaveStyle({ visibility: "hidden" });
expect(getLatestTerminalInstance()).toBe(initialRenderer);
expect(initialRenderer.dispose).not.toHaveBeenCalled();
expect(mockInvoke.mock.calls.filter(
  ([command]) => command === "request_terminal_snapshot",
)).toHaveLength(initialSnapshotRequests);
expect(mockInvoke).toHaveBeenCalledWith(
  "update_terminal_presentation",
  expect.objectContaining({
    request: expect.objectContaining({
      visibility: "hidden",
      render_state: "mounted",
    }),
  }),
);
```

After showing again, assert the renderer object is unchanged and no registration, rebind, or snapshot request was added.

- [ ] **Step 2: Run the test and verify immediate retirement**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx --testNamePattern "retains a hidden mounted renderer"
```

Expected: FAIL because visibility currently participates in renderer lifecycle activity and immediately calls `retireRenderer`.

- [ ] **Step 3: Make render state own xterm residency**

Replace the visibility-coupled residency branch with these rules:

```ts
const shouldRetainRenderer = renderState === "mounted";
const shouldRevealRenderer = shouldRetainRenderer && visibility === "visible";

if (!shouldRetainRenderer) {
  invalidateReveal();
  retireCurrentRenderer();
} else if (!shouldRevealRenderer) {
  invalidateReveal();
  markRendererReady(false);
} else if (!entry?.renderer) {
  setRendererMountRevision((revision) => revision + 1);
}
```

The main attach effect must return without creating a renderer for an initially hidden presentation, but it must leave an already mounted renderer and subscription intact. Continue sending `visibility="hidden"` and `render_state="mounted"` to the broker so hidden input remains rejected by broker authority.

- [ ] **Step 4: Run terminal lifecycle tests**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx --testNamePattern "retains a hidden mounted renderer|suspends|ownership|input"
```

Expected: PASS with the same xterm identity across hide/show and no new snapshot request.

- [ ] **Step 5: Commit the renderer residency lifecycle**

```powershell
git add -- src/features/terminal/AgentTerminal.tsx src/features/terminal/AgentTerminal.test.tsx
git commit -m "fix(terminal): retain hidden mounted renderers"
```

---

### Task 3: Replace correction fits with a generation-aware reveal barrier

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`

**Interfaces:**
- Consumes: renderer identity, presentation lifecycle refs, current host bounds, xterm cell metrics, physical intersection, `document.fonts.ready`, and awaited broker snapshot writes.
- Produces: `beginRendererReveal(): number`, `invalidateRendererReveal(): void`, and `prepareRendererForReveal(...): Promise<boolean>` semantics local to AgentTerminal; only the latest generation calls `markRendererReady(true)`.
- Produces: an optional `beforeInitialSnapshot` registration hook that runs after broker identity is established but before the initial snapshot callback mutates xterm.

- [ ] **Step 1: Write failing preparation-order tests**

Add tests with deferred snapshot writes and controlled IntersectionObserver/WebGL callbacks. Assert:

```ts
expect(host).toHaveStyle({ visibility: "hidden" });
expect(terminal.resize).toHaveBeenCalledTimes(1);
expect(terminal.refresh).not.toHaveBeenCalled();

resolveSnapshotWrite();
await waitFor(() => expect(host).toHaveStyle({ visibility: "visible" }));
expect(revealOrder).toEqual(["backend", "fit", "snapshot-complete", "reveal"]);
```

Add a stale-generation test that begins preparation, suspends/replaces the renderer before a deferred font or snapshot promise resolves, then proves the old operation never reveals or refreshes the replacement.

- [ ] **Step 2: Run the tests and verify premature reveal and extra refreshes**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx --testNamePattern "reveals only after settled first paint|stale reveal generation"
```

Expected: FAIL because the current `prepareRendererForReveal` performs fit/refresh/fit and the host can become ready before WebGL and later correction fits finish.

- [ ] **Step 3: Add reveal generation ownership**

Add refs and callbacks with exact current-generation checks:

```ts
const revealGenerationRef = useRef(0);
const physicalIntersectionRef = useRef(typeof IntersectionObserver === "undefined");

const invalidateRendererReveal = useCallback(() => {
  revealGenerationRef.current += 1;
  markRendererReady(false);
}, [markRendererReady]);

const beginRendererReveal = useCallback(() => {
  const generation = revealGenerationRef.current + 1;
  revealGenerationRef.current = generation;
  markRendererReady(false);
  return generation;
}, [markRendererReady]);
```

Every continuation checks generation, renderer identity, connected non-zero host bounds, visible/mounted lifecycle, and physical intersection before mutation.

- [ ] **Step 4: Add a serialized pre-snapshot registration hook**

Extend the client without splitting its serialization boundary:

```ts
type RegisterPresentationOptions = {
  beforeInitialSnapshot?: (
    result: TerminalPresentationRegistrationResult,
  ) => Promise<void>;
};

async registerPresentation(
  registration: TerminalPresentationRegistration,
  callbacks: TerminalPresentationCallbacks,
  options?: RegisterPresentationOptions,
) {
  // Existing invoke and binding validation remain unchanged.
  await options?.beforeInitialSnapshot?.(result);
  await this.#applySnapshot(binding, result.initial_snapshot);
  // Existing subscription setup remains unchanged.
}
```

Add a client test with a deferred hook and assert `applySnapshot` is not called until the hook resolves, while a concurrent drain remains serialized behind registration.

- [ ] **Step 5: Implement stable-sample preparation**

Introduce an internal sample with exact equality:

```ts
type TerminalRevealSample = {
  width: number;
  height: number;
  cellWidth: number;
  cellHeight: number;
  cols: number;
  rows: number;
  backend: "webgl" | "dom";
};
```

`prepareRendererForReveal` must:

1. begin a generation and stay hidden;
2. await `document.fonts.ready` only when `document.fonts.status === "loading"`;
3. await the first physical IntersectionObserver decision and complete one WebGL promotion attempt or DOM fallback;
4. sample bounds/cells/backend;
5. call `fitTerminalToContainer` once;
6. let initial registration apply its snapshot through the new pre-snapshot hook, or request an eviction-recovery snapshot after the prefit;
7. await the existing xterm write queue with `renderer.term.write("", resolve)`;
8. sample again;
9. repeat while hidden only when the samples differ, capped at three synchronous layout attempts;
10. reveal only when the final sample matches the terminal grid and the generation is current.

Remove `term.refresh()` from reveal preparation; xterm resize/backend activation already schedules rendering. A failed WebGL load leaves `backend: "dom"` and remains eligible to reveal.

- [ ] **Step 6: Put backend choice before snapshot and reveal**

Install IntersectionObserver immediately after the renderer host is connected. Its callback updates `physicalIntersectionRef`, performs at most one WebGL promotion attempt, and resolves the backend-ready portion of preparation:

```ts
physicalIntersectionRef.current = lastObservation.isIntersecting;
if (lastObservation.isIntersecting) {
  promoteSessionToWebgl(terminalKey);
  resolveBackendReady();
} else {
  invalidateRendererReveal();
  resolveBackendReady(); // DOM is the settled offscreen backend.
}
```

Pass `awaitBackendReadyAndFit` as `beforeInitialSnapshot` for a new registration. For an evicted renderer, call it before `requestPresentationSnapshot`. When IntersectionObserver is unavailable, activate/fallback the backend synchronously before the first fit. After the awaited snapshot completes, validate the final sample and reveal. A later intersection of a registered offscreen renderer begins a new hidden preparation generation before revealing it.

- [ ] **Step 7: Remove unconditional initial correction paths**

Delete the initial next-frame fit, 50 ms fit, 300 ms unchanged report, and post-WebGL refit. Do not replace them with another timer. The registration path calls the reveal barrier once after backend selection.

- [ ] **Step 8: Run the preparation tests**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx src/features/terminal/terminalSessionClient.test.ts --testNamePattern "first paint|reveal|WebGL|font|snapshot|before initial snapshot"
```

Expected: PASS; one settled grid is visible and stale generations cannot mutate replacements.

- [ ] **Step 9: Commit the reveal barrier**

```powershell
git add -- src/features/terminal/AgentTerminal.tsx src/features/terminal/AgentTerminal.test.tsx src/features/terminal/terminalSessionClient.ts src/features/terminal/terminalSessionClient.test.ts
git commit -m "fix(terminal): gate reveal on settled first paint"
```

---

### Task 4: Coalesce genuine post-reveal geometry changes once

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`

**Interfaces:**
- Consumes: `fitTerminalToContainer`, `lastMeasuredHostSize`, current renderer generation, ResizeObserver, and terminal appearance settings.
- Produces: one animation-frame fit scheduler that ignores unchanged geometry and invalidates/reprepares only for real cell-metric or backend changes.

- [ ] **Step 1: Tighten failing ResizeObserver and appearance tests**

Extend the existing burst tests so three same-size observations produce no additional resize, refresh, report, or PTY resize. Three changed-size observations in one frame produce exactly one fit and one final grid commit. Add an initial appearance test proving the renderer's constructor settings do not trigger a redundant mount refresh/refit.

- [ ] **Step 2: Run the tests and verify the debounce/double-fit behavior**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx --testNamePattern "ResizeObserver|appearance|font"
```

Expected: FAIL because the current observer uses a 250 ms timeout followed by both `checkSizing()` and a second animation-frame `performFit()`.

- [ ] **Step 3: Replace the timeout cascade with one frame scheduler**

Track one frame on the renderer:

```ts
const scheduleMeasuredFit = () => {
  if (renderer.fitFrame !== null) return;
  renderer.fitFrame = requestAnimationFrame(() => {
    renderer.fitFrame = null;
    void checkSizing();
  });
};
```

Rename the renderer field from `fitTimeout` to `fitFrame`, cancel it with `cancelAnimationFrame`, and call only `scheduleMeasuredFit()` from ResizeObserver. `fitTerminalToContainer` retains its exact measured-size and proposed-grid equality checks, so unchanged frames do not report geometry.

- [ ] **Step 4: Make appearance changes evidence-driven**

Change `applyTerminalAppearance` to compare current `term.options.fontSize` and `fontFamily` first. If neither changed, return without refresh or fit. If either changed, update options, invalidate the reveal generation, and run the same preparation barrier; do not call `refresh()` followed by a separate next-frame fit.

- [ ] **Step 5: Run the complete terminal unit suite**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx src/views/AgentsOverviewView.test.tsx
```

Expected: PASS with one coalesced changed-size fit and zero unchanged-size work.

- [ ] **Step 6: Commit deterministic post-reveal fitting**

```powershell
git add -- src/features/terminal/AgentTerminal.tsx src/features/terminal/AgentTerminal.test.tsx
git commit -m "fix(terminal): coalesce measured renderer fits"
```

---

### Task 5: Prove first-paint stability in the native application

**Files:**
- Modify: `e2e-native/helpers/terminal-debug.mjs`
- Create: `e2e-native/tests/terminal-first-paint-native.test.mjs`
- Modify: `docs/developer/terminal-presentation-broker.md`
- Modify: `docs/guide/workbench.md`

**Interfaces:**
- Consumes: `window.__wardianTerminalDebug.snapshot(presentationId)`, stable Agents presentation IDs, workbench tab helpers, and isolated mock-provider homes.
- Produces: native evidence for renderer reuse, ready-state ordering, stable geometry, content preservation, and evicted-renderer reconstruction.

- [ ] **Step 1: Add debug fields needed for behavioral assertions**

Expose monotonic, non-sensitive diagnostics from AgentTerminal's existing debug snapshot:

```ts
renderer: {
  instanceId: renderer.instanceId,
  ready: rendererReadyRef.current,
  revealGeneration: revealGenerationRef.current,
  fitCount: entry.fitCount,
  resizeCount: entry.resizeCount,
  cols: renderer.term.cols,
  rows: renderer.term.rows,
}
```

Update `e2e-native/helpers/terminal-debug.mjs` to return those fields without parsing DOM text.

- [ ] **Step 2: Write the native switch regression**

Seed four live mock-provider agents in an isolated home. Open Agents, wait for every visible host to report ready, capture renderer IDs and final grids, switch to Workflows, then back to Agents five times. On every return assert:

```js
assert.equal(snapshot.renderer.instanceId, initial.instanceId);
assert.deepEqual(
  { cols: snapshot.renderer.cols, rows: snapshot.renderer.rows },
  initial.grid,
);
assert.equal(snapshot.renderer.ready, true);
const fitDelta = snapshot.fitCount - previous.fitCount;
assert.ok(fitDelta >= 0 && fitDelta <= 1);
```

Sample the host during each transition and fail if it is visible while its grid differs from the final grid. Confirm the mock terminal marker, owner presentation ID, and scroll position remain unchanged. Then exceed the renderer budget or invoke the existing debug eviction hook for one presentation and prove its new renderer remains hidden until ready.

- [ ] **Step 3: Run unit and source-level native checks first**

Run:

```powershell
npx vitest run src/features/terminal/AgentTerminal.test.tsx src/views/AgentsOverviewView.test.tsx
node --test e2e-native/tests/rendering-audit-options.test.mjs
```

Expected: PASS.

- [ ] **Step 4: Rebuild the native debug application**

Run:

```powershell
$env:VITE_WARDIAN_TERMINAL_DEBUG = '1'
$env:CARGO_BUILD_JOBS = '1'
npm run tauri -- build --debug --no-bundle
```

Expected: `target/debug/Wardian.exe` builds successfully.

- [ ] **Step 5: Run the native first-paint and lifecycle tests**

Run:

```powershell
$env:WARDIAN_NATIVE_SKIP_BUILD = '1'
npm run test:e2e:native:fast -- e2e-native/tests/terminal-first-paint-native.test.mjs e2e-native/tests/workbench-runtime-lifecycle-native.test.mjs e2e-native/tests/terminal-visibility-snapshot-native.test.mjs
```

Expected: all native tests PASS with no visible pre-settle frame.

- [ ] **Step 6: Document the implemented lifecycle**

Update the broker developer guide with the hidden/mounted residency distinction and reveal ordering. Update the Workbench guide to explain that short tab switches reuse budgeted terminal presentations and that an evicted terminal is shown only after restoration completes.

- [ ] **Step 7: Run the complete project gates**

Run:

```powershell
npm run lint
npm run test
npm run build
Push-Location src-tauri
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all -- --test-threads=1
cargo check --all-targets --all-features
Pop-Location
git diff --check
git status --short
```

Expected: every command passes; `git status --short` lists only the user's pre-existing `package-lock.json` modification before the task files are staged.

- [ ] **Step 8: Capture and embed feature evidence**

Capture the settled Agents surface under `e2e/screenshots/terminal-first-paint/2026-07-15/agents-settled.png`, commit it explicitly if ignored, and embed its raw GitHub HTTPS URL in PR #667.

- [ ] **Step 9: Commit documentation, native coverage, and screenshot evidence**

```powershell
git add -- e2e-native/helpers/terminal-debug.mjs e2e-native/tests/terminal-first-paint-native.test.mjs docs/developer/terminal-presentation-broker.md docs/guide/workbench.md
git add -f -- e2e/screenshots/terminal-first-paint/2026-07-15/agents-settled.png
git commit -m "test(terminal): prove stable first paint"
```

- [ ] **Step 10: Push and update the pull request**

```powershell
git push origin feat/navigation-workbench
gh pr checks 667
```

Expected: the branch pushes successfully and PR #667 reports the new checks as queued or passing.
