# Pane-Responsive Surfaces Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore useful Agents geometry and make every Workbench surface render without overlap or horizontal overflow in a partial-width pane.

**Architecture:** Agents Auto will distinguish preferred card geometry from its hard renderer floor and will use one numeric grid-gap source for layout and resize affordances. Dockview surface panels will become CSS inline-size query containers; semantic component classes and a small amount of Library state-aware rendering will adapt each surface to its pane without coupling it to global viewport width.

**Tech Stack:** React 19, TypeScript, Zustand, Tailwind utility classes, native CSS container queries, Vitest/Testing Library, Playwright, Dockview.

## Global Constraints

- Terminal preferred size is 640 x 450px; terminal hard floor remains 520 x 280px.
- Chat preferred size is 480 x 450px; chat hard floor remains 360 x 280px.
- Explicit Single is the only Agents mode that hides the rest of the roster.
- Responsive decisions use the live Dockview pane width, not `window.innerWidth` or viewport media queries.
- Use Wardian theme variables/classes; add no hardcoded Tailwind palette colors.
- Preserve terminal renderer identity, ownership, and PTY lifecycle behavior.
- Preserve the unrelated working-tree modification in `package-lock.json`.

---

### Task 1: Preferred Agents Auto geometry

**Files:**
- Modify: `src/features/grid/agentsOverviewLayout.ts`
- Test: `src/features/grid/agentsOverviewLayout.test.ts`
- Test: `src/views/AgentsOverviewView.test.tsx`

**Interfaces:**
- Consumes: `AgentsOverviewLayoutAgent`, `AgentsOverviewContainerSize`, and `resolveAgentsOverviewLayout`.
- Produces: exported `TERMINAL_CARD_PREFERRED`, `CHAT_CARD_PREFERRED`, and Auto layout results whose `cardHeight` is selected around the 450px preferred height while retaining every visible agent ID.

- [ ] **Step 1: Add failing preferred-size tests**

Add terminal and chat cases that assert a 950px-tall pane selects two 450px preferred rows, a 1,400px-tall pane can select three 450px preferred rows, and a constrained pane compresses rows only as far as the 280px floor. Assert `visibleAgentIds` still contains the complete roster and `requiresScroll` is true when rows extend past the viewport.

```ts
expect(resolveAgentsOverviewLayout({
  mode: "auto",
  agents: terminalAgents(6),
  containerSize: { width: 700, height: 950 },
}).cardHeight).toBe(450);

expect(resolveAgentsOverviewLayout({
  mode: "auto",
  agents: terminalAgents(6),
  containerSize: { width: 700, height: 950 },
}).visibleAgentIds).toHaveLength(6);
```

- [ ] **Step 2: Run the focused tests and confirm the old floor-packed result fails**

Run: `npm run test -- --run src/features/grid/agentsOverviewLayout.test.ts src/views/AgentsOverviewView.test.tsx`

Expected: the 950px case reports roughly 311px card height instead of the preferred two-row result.

- [ ] **Step 3: Implement preferred row selection**

Add preferred constants and select the visible row count nearest the preferred height while clamping it to the number of hard-floor rows that fit:

```ts
export const TERMINAL_CARD_PREFERRED = Object.freeze({ width: 640, height: 450 });
export const CHAT_CARD_PREFERRED = Object.freeze({ width: 480, height: 450 });

function preferredViewportRows(height: number, preferredHeight: number, floorHeight: number, gap: number): number {
  const innerHeight = Math.max(0, height - (2 * gap));
  const hardFloorCapacity = Math.max(1, Math.floor((innerHeight + gap) / (floorHeight + gap)));
  const ideal = Math.max(1, Math.round((innerHeight + gap) / (preferredHeight + gap)));
  return Math.min(hardFloorCapacity, ideal);
}
```

Use the strictest preferred/floor dimensions across mixed terminal/chat cards. The chosen card height is the preferred height when it fits; otherwise it is the available inner height per chosen row, clamped at the floor. Keep the existing column admission and hysteresis logic.

- [ ] **Step 4: Run the focused layout tests**

Run: `npm run test -- --run src/features/grid/agentsOverviewLayout.test.ts src/views/AgentsOverviewView.test.tsx`

Expected: all focused tests pass.

- [ ] **Step 5: Commit the Agents Auto slice**

```bash
git add src/features/grid/agentsOverviewLayout.ts src/features/grid/agentsOverviewLayout.test.ts src/views/AgentsOverviewView.test.tsx
git commit -m "fix(agents): prefer useful auto card geometry"
```

### Task 2: Exact Grid gutter geometry

**Files:**
- Modify: `src/features/grid/agentsOverviewLayout.ts`
- Modify: `src/views/AgentsOverviewView.tsx`
- Test: `src/views/AgentsOverviewView.test.tsx`
- Test: `e2e/tests/workbench-overview.spec.ts`

**Interfaces:**
- Consumes: the exported numeric `DEFAULT_AGENTS_OVERVIEW_GAP` and persisted `manualLayout.row_height`.
- Produces: `agentsOverviewGridRowOrigin(rowIndex, rowHeight, gap)` and `agentsOverviewGridRowBoundary(rowIndex, rowHeight, gap)`, used by both resize-handle families.

- [ ] **Step 1: Add failing gutter-offset tests**

For a 450px row and 6px gap/padding, assert card origins are `6`, `462`, and `918`, while boundaries after the first and second rows are `456` and `912`. Render three rows and assert the handle styles use these values rather than accumulating an 8px assumption.

```ts
expect(agentsOverviewGridRowOrigin(2, 450, 6)).toBe(918);
expect(agentsOverviewGridRowBoundary(1, 450, 6)).toBe(912);
```

- [ ] **Step 2: Run the focused view test and confirm drift remains**

Run: `npm run test -- --run src/views/AgentsOverviewView.test.tsx`

Expected: later row handles report the old offset.

- [ ] **Step 3: Implement one numeric grid geometry source**

Set `DEFAULT_AGENTS_OVERVIEW_GAP` to the current Wardian density value of `6`, feed that value to the layout hook, grid `gap`, grid `padding`, column math, row-origin helpers, and gutter styles. Position a 12px gutter by subtracting 6px from its exact boundary.

```ts
export function agentsOverviewGridRowOrigin(rowIndex: number, rowHeight: number, gap = DEFAULT_AGENTS_OVERVIEW_GAP): number {
  return gap + (rowIndex * (rowHeight + gap));
}

export function agentsOverviewGridRowBoundary(rowIndex: number, rowHeight: number, gap = DEFAULT_AGENTS_OVERVIEW_GAP): number {
  return agentsOverviewGridRowOrigin(rowIndex, rowHeight, gap) + rowHeight;
}
```

- [ ] **Step 4: Add a browser scroll regression**

In explicit Grid with at least five one-column cards, scroll to a later agent and compare the hovered gutter center to the adjacent card boundary with a tolerance of 1px.

- [ ] **Step 5: Run focused unit and browser tests**

Run: `npm run test -- --run src/views/AgentsOverviewView.test.tsx`

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/workbench-overview.spec.ts`

Expected: all focused tests pass and later gutters stay aligned after scrolling.

- [ ] **Step 6: Commit the Grid geometry slice**

```bash
git add src/features/grid/agentsOverviewLayout.ts src/views/AgentsOverviewView.tsx src/views/AgentsOverviewView.test.tsx e2e/tests/workbench-overview.spec.ts
git commit -m "fix(agents): align grid resize gutters"
```

### Task 3: Workbench container contract and CSS-responsive surfaces

**Files:**
- Modify: `src/layout/workbench/workbench.css`
- Modify: `src/styles/App.css`
- Modify: `src/views/DashboardView.tsx`
- Modify: `src/views/GraphView.tsx`
- Modify: `src/features/workbench/HomeSurface.tsx`
- Test: `src/features/workbench/HomeSurface.test.tsx`
- Test: `src/views/GraphView.test.tsx`

**Interfaces:**
- Consumes: `.wardian-workbench-surface-panel` from `DockviewSurfacePanel`.
- Produces: named container `wardian-surface` and semantic child classes for Dashboard, Graph, and New Tab.

- [ ] **Step 1: Add failing container-contract tests**

Assert the Workbench stylesheet declares `container-type: inline-size` and `container-name: wardian-surface`. Add component assertions for semantic Dashboard and Graph child classes used by compact queries.

- [ ] **Step 2: Run the focused tests**

Run: `npm run test -- --run src/features/workbench/HomeSurface.test.tsx src/views/GraphView.test.tsx`

Expected: container and semantic-class assertions fail.

- [ ] **Step 3: Implement the container contract**

```css
.wardian-workbench-surface-panel {
  container-name: wardian-surface;
  container-type: inline-size;
}
```

Replace the New Tab and Graph viewport media rules with `@container wardian-surface (...)`. Give Dashboard's card layout semantic classes and replace `md:` viewport variants with container rules that stack metadata and actions inside compact panes.

- [ ] **Step 4: Run focused tests**

Run: `npm run test -- --run src/features/workbench/HomeSurface.test.tsx src/views/GraphView.test.tsx src/views/DashboardView.test.tsx`

Expected: all focused tests pass.

- [ ] **Step 5: Commit the base responsive contract**

```bash
git add src/layout/workbench/workbench.css src/styles/App.css src/views/DashboardView.tsx src/views/GraphView.tsx src/features/workbench/HomeSurface.tsx src/features/workbench/HomeSurface.test.tsx src/views/GraphView.test.tsx src/views/DashboardView.test.tsx
git commit -m "fix(workbench): make surfaces pane responsive"
```

### Task 4: Compact Workflows and Library

**Files:**
- Modify: `src/views/WorkflowsView.tsx`
- Test: `src/views/WorkflowsView.test.tsx`
- Modify: `src/views/LibraryView.tsx`
- Test: `src/views/LibraryView.test.tsx`
- Modify: `src/styles/App.css`

**Interfaces:**
- Consumes: the `wardian-surface` container and `useLibraryStore.select(entryRef)`.
- Produces: semantic Workflows toolbar/body/drawer classes and Library `data-detail-open` plus `Back to library list` control.

- [ ] **Step 1: Add failing Workflows structure tests**

Assert toolbar primary/actions groups, run drawer, edit body, inspector, and launch overlay expose semantic classes. Assert all actions remain present and ordered.

- [ ] **Step 2: Add failing Library compact-navigation tests**

Seed a selected Library entry, assert the root exposes `data-detail-open="true"`, click `Back to library list`, and assert `select(null)` is called. Assert the Back control remains hidden from regular layout through its semantic class, not conditional viewport JavaScript.

- [ ] **Step 3: Run the focused tests and confirm failure**

Run: `npm run test -- --run src/views/WorkflowsView.test.tsx src/views/LibraryView.test.tsx`

Expected: semantic-class and Back-control assertions fail.

- [ ] **Step 4: Implement compact Workflows behavior**

Add semantic classes and container rules so the toolbar wraps into primary and action rows. At compact width, make the run drawer and node inspector absolute right-side drawers with bounded width `min(320px, 92cqw)`, full available height, border, and elevated stacking. Clamp launch/node-library overlay padding to 12px.

- [ ] **Step 5: Implement compact Library browse/detail behavior**

Read `selection` and `select` in `LibraryView`. Add `data-detail-open`, semantic list/detail classes, and a `Back to library list` button that calls `void select(null)`. Container rules keep the three-pane layout at regular width; below the narrow breakpoint they show the browse state until an item is selected, then show the full-width detail state. Existing `DetailPane` selection-change guards continue to handle dirty drafts.

- [ ] **Step 6: Run focused tests**

Run: `npm run test -- --run src/views/WorkflowsView.test.tsx src/views/LibraryView.test.tsx`

Expected: all focused tests pass.

- [ ] **Step 7: Commit Workflows and Library**

```bash
git add src/views/WorkflowsView.tsx src/views/WorkflowsView.test.tsx src/views/LibraryView.tsx src/views/LibraryView.test.tsx src/styles/App.css
git commit -m "fix(workbench): adapt complex surfaces to narrow panes"
```

### Task 5: Cross-surface split-pane proof and delivery

**Files:**
- Create: `e2e/tests/workbench-responsive-surfaces.spec.ts`
- Create: `e2e/screenshots/pane-responsive-surfaces/2026-07-15/workflows-compact.png`
- Modify: `docs/specs/2026-07-15-pane-responsive-surfaces.md` only if implementation evidence changes an explicit decision.

**Interfaces:**
- Consumes: Workbench IPC fixtures, `surfacePanel`, and every core non-agent surface type.
- Produces: browser proof that Dashboard, Queue, Graph, Garden, Library, Workflows, and New Tab tolerate partial-width panes.

- [ ] **Step 1: Add the split-pane matrix**

Boot real horizontal split documents at 1440 x 900. For each surface, assert its root has `scrollWidth <= clientWidth + 1`, primary controls remain within its bounding box, and canvas surfaces have non-zero geometry. For Workflows, assert toolbar buttons do not overlap and compact drawers remain closable.

- [ ] **Step 2: Run the new browser test**

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/workbench-responsive-surfaces.spec.ts`

Expected: every surface passes the partial-pane matrix.

- [ ] **Step 3: Capture feature evidence**

Capture compact Workflows and Agents Auto/Grid states under `e2e/screenshots/pane-responsive-surfaces/2026-07-15/`. Inspect each image before committing and retain only screenshots that demonstrate the changed geometry.

- [ ] **Step 4: Run required frontend validation**

Run: `npm run lint`

Run: `npm run test`

Run: `npm run build`

Run: `npm run test:e2e`

Expected: lint/build succeed; all unit and browser suites pass with only intentional native-only skips.

- [ ] **Step 5: Run native terminal smoke**

Run the isolated native Workbench/terminal smoke appropriate to current debug artifacts. Confirm Auto fits live renderers and does not introduce Terminal Initialization Fatal Error, ownership changes, or renderer flicker.

- [ ] **Step 6: Run repository integrity checks**

Run: `git diff --check`

Run: `git status --short`

Confirm `package-lock.json` remains the user's unrelated unstaged modification and no secret-bearing files are included.

- [ ] **Step 7: Commit, push, and update PR #667**

```bash
git add e2e/tests/workbench-responsive-surfaces.spec.ts e2e/screenshots/pane-responsive-surfaces/2026-07-15
git commit -m "test(workbench): prove partial-pane surfaces"
git push origin feat/navigation-workbench
```

Embed at least one new HTTPS screenshot in PR #667 and record the focused/full validation results.
