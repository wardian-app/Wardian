# Agents Workbench UX Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the original Agents grid, fuse workbench tabs into Obsidian-style window chrome, and make reclaimed terminals restore and fit automatically.

**Architecture:** Recompose `AppShell` so Dockview's real top-edge group headers occupy the main titlebar segment instead of projecting duplicate tabs. Restore original grid geometry as the Agents grid branch, constrain Auto to grid-or-single, and separate terminal restoration from owner-only PTY sizing.

**Tech Stack:** React 19, TypeScript, Dockview React 7, Xterm, Zustand, Tauri 2, Vitest, Playwright.

## Global Constraints

- Keep the internal `agents-overview` surface identifier compatible with persisted workbench documents.
- Use semantic Wardian theme variables; do not introduce hardcoded UI colors.
- Preserve Dockview-owned tab roles, drag/drop, overflow, and focus behavior.
- Keep the top chrome height at 36 pixels.
- Never allow a mirror presentation to resize the shared PTY.
- Remove `Activate terminal renderer` from runtime UI, source assertions, and user guides.

---

### Task 1: Fuse workbench headers into the window chrome

**Files:**
- Modify: `src/layout/AppShell.tsx`
- Modify: `src/layout/titlebar/CustomTitleBar.tsx`
- Modify: `src/layout/titlebar/CustomTitleBar.test.tsx`
- Modify: `src/views/App.tsx`
- Modify: `src/styles/App.css`
- Modify: `src/layout/workbench/DockviewLayoutAdapter.tsx`
- Modify: `src/layout/workbench/WorkbenchGroupHeader.tsx`
- Modify: `src/layout/workbench/WorkbenchTab.tsx`
- Modify: `src/layout/workbench/workbench.css`
- Test: `src/layout/workbench/DockviewLayoutAdapter.test.tsx`
- Test: `src/layout/workbench/WorkbenchHost.test.tsx`

**Interfaces:**
- Consumes: canonical workbench document and Dockview group headers.
- Produces: a shell whose workbench begins at the window top and group actions split between Dockview's after-tabs and right-edge slots.

- [ ] Write failing component tests asserting no empty center titlebar, plus immediately after the tab list, pane actions at the far edge, and interactive controls outside native drag regions.
- [ ] Run `npm run test -- src/layout/titlebar/CustomTitleBar.test.tsx src/layout/workbench/DockviewLayoutAdapter.test.tsx src/layout/workbench/WorkbenchHost.test.tsx` and confirm the new assertions fail.
- [ ] Recompose the shell and split New Surface from pane actions while preserving Dockview's real header DOM.
- [ ] Replace button-like tab chrome with compact Obsidian-style tab surfaces and responsive overflow.
- [ ] Rerun the focused tests and commit `feat(workbench): fuse tabs into window chrome`.

### Task 2: Restore the Agents grid policy and public naming

**Files:**
- Modify: `src/features/grid/agentsOverviewLayout.ts`
- Test: `src/features/grid/agentsOverviewLayout.test.ts`
- Modify: `src/views/AgentsOverviewView.tsx`
- Test: `src/views/AgentsOverviewView.test.tsx`
- Modify: `src/features/workbench/coreSurfaceRegistry.ts`
- Modify: `src/features/workbench/surfaces/AgentsOverviewSurface.tsx`
- Modify: related workbench and App tests containing user-facing `Agents Overview` copy.

**Interfaces:**
- Consumes: persisted `AgentsOverviewMode`, manual grid tracks, row height, focused agent, and measured surface bounds.
- Produces: Auto results limited to original-style grid or singleton and the public title `Agents`.

- [ ] Write failing layout tests proving wide Auto uses a balanced multi-column grid, narrow Auto uses a singleton, explicit Grid preserves manual columns, and Auto never emits a one-column multi-agent grid.
- [ ] Run the focused layout and view tests and confirm failure.
- [ ] Restore original grid track and row-height behavior in `AgentsOverviewView` and simplify the Auto policy.
- [ ] Update public surface titles and test queries to `Agents` without changing persisted type identifiers.
- [ ] Rerun focused tests and commit `fix(agents): restore the original monitoring grid`.

### Task 3: Make terminal restoration automatic and geometry owner-safe

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`
- Create or modify focused terminal lifecycle/geometry helpers only if extraction reduces the existing component's mixed responsibilities.
- Modify: `src/features/terminal/terminalSessionRegistry.ts` and tests if owner-only resize enforcement belongs at the registry boundary.

**Interfaces:**
- Consumes: presentation visibility, renderer eviction state, measured container geometry, and broker ownership.
- Produces: automatic renderer restoration and PTY resize calls emitted only by the interactive owner.

- [ ] Replace the activation-button test with failing tests for automatic visible restoration, hidden non-restoration, correctly ordered fit/reveal, Retry on failure, and mirror resize suppression.
- [ ] Run `npm run test -- src/features/terminal/AgentTerminal.test.tsx src/features/terminal/terminalSessionRegistry.test.ts` and confirm failure.
- [ ] Implement automatic restoration triggered by visible nonzero geometry and remove the activation overlay.
- [ ] Route resize through one observer-driven fitting path and enforce owner-only PTY resize submission.
- [ ] Rerun focused tests and commit `fix(terminal): restore visible renderers automatically`.

### Task 4: Documentation, browser behavior, and screenshots

**Files:**
- Modify: `docs/guide/agents-overview.md`
- Modify: `docs/guide/getting-started.md`
- Modify: `docs/guide/grid.md`
- Modify: `docs/guide/ui-overview.md`
- Modify: `docs/guide/workbench.md`
- Modify: `e2e/tests/workbench-navigation.spec.ts`
- Modify: `e2e/tests/workbench-overview.spec.ts`
- Add: `e2e/screenshots/workbench-navigation/<timestamp>/*.png`

**Interfaces:**
- Consumes: completed chrome, grid, and renderer behaviors.
- Produces: browser regression coverage and feature-specific visual evidence.

- [ ] Update guides to describe Agents, fused tabs, Auto/Grid/Single, automatic renderer restoration, and owner-safe sizing.
- [ ] Add browser assertions for adjacent plus, top-edge split headers, lower split headers, wide grid, and narrow singleton.
- [ ] Run focused browser E2E and fix any behavioral regressions.
- [ ] Capture representative screenshots and commit `docs(workbench): document repaired navigation UX`.

### Task 5: Native and full verification

**Files:**
- Modify: native terminal tests only when coverage gaps require it.
- Modify: PR body with an embedded HTTPS screenshot after upload.

**Interfaces:**
- Consumes: the integrated branch.
- Produces: merge-ready validation evidence.

- [ ] Run `npm run lint`, `npm run test`, `npm run build`, and `npm run test:e2e`.
- [ ] Run targeted native terminal and shared-state E2E, then `npm run test:e2e:native` when the harness is available.
- [ ] Run `cargo clippy`, `cargo test`, and `cargo check` in `src-tauri`.
- [ ] Run secret and status checks, inspect the final diff, and obtain a whole-branch review.
- [ ] Push the branch, embed a representative screenshot in PR #667, and verify CI.

