# Terminal Renderer Lifecycle and New Tab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate terminal and Agents layout regressions, make `+` open the existing surface picker as an inline New Tab, and prevent pane drags from promising impossible splits.

**Architecture:** Terminal renderer retirement becomes a lease-aware operation that defers physical xterm disposal until in-flight asynchronous renderer work settles. Agents uses stable renderer residency while xterm's own observer independently budgets WebGL. A registered `new-tab` surface gives the visual launcher ordinary tab identity and replacement semantics.

**Tech Stack:** React 19, TypeScript, Vitest, Dockview, xterm.js, Playwright, Tauri native E2E.

## Global Constraints

- Preserve the backend as the authority for PTY lifecycle and canonical geometry.
- Do not restart or interfere with the user's live Wardian client.
- Use semantic theme variables for UI styling.
- Keep the direct-palette `+` preference and keyboard Quick Open behavior.

---

### Task 1: Renderer ownership barrier and scroll repaint

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`

**Interfaces:**
- Produces: renderer retirement fields on `TerminalRendererEntry` and an async renderer-operation helper that returns whether the same renderer remains current.
- Produces: wheel handling that delegates repaint to xterm's `scrollLines` implementation.

- [ ] Add a delayed-write regression test that starts broker snapshot output, retires the renderer before the xterm callback resolves, then proves the write settles before disposal and no fatal overlay appears.
- [ ] Run `npm run test -- --run src/features/terminal/AgentTerminal.test.tsx` and confirm the new test fails on the mutable `entry.renderer` dereference.
- [ ] Add explicit renderer retirement state and in-flight operation accounting; release budget ownership immediately but defer physical `term.dispose()` until all acquired operations settle.
- [ ] Capture one renderer for each async output sequence and stop post-write refresh/scroll work when that renderer lease was retired or replaced.
- [ ] Remove Wardian's explicit `refresh()` after `scrollLines()` and change the wheel test to reject a duplicate refresh.
- [ ] Re-run the focused terminal test and confirm it passes.

### Task 2: Stable Agents renderer residency

**Files:**
- Modify: `src/views/AgentsOverviewView.tsx`
- Modify: `src/features/grid/agentsOverviewLayout.ts`
- Modify: `src/features/workbench/surfaces/AgentsOverviewSurface.tsx`
- Modify: `src/types/index.ts`
- Test: `src/views/AgentsOverviewView.test.tsx`
- Test: `src/features/grid/agentsOverviewLayout.test.ts`
- Test: `src/features/workbench/surfaces/AgentsOverviewSurface.test.tsx`

**Interfaces:**
- Consumes: `MAX_XTERM_RENDERERS`.
- Produces: a stable set of resident agent presentation ids; physical intersection no longer directly means renderer destruction.
- Produces: capacity-based Auto layout with vertical overflow and persisted `last_multi_agent_mode: "auto" | "grid"` focus restoration.

- [ ] Add tests proving a grid at or below capacity keeps every terminal mounted across intersection exit/re-entry and a grid above capacity only evicts a non-near resident when admitting a new near card.
- [ ] Run `npm run test -- --run src/views/AgentsOverviewView.test.tsx` and confirm the continuity test fails.
- [ ] Replace the transient intersection set with stable residency: seed all ids at or below capacity; above capacity retain residents and admit approaching ids by removing non-near residents first.
- [ ] Replace Auto's all-roster compression score with viewport capacity: when at least two terminal-floor cards fit side by side, preserve their floor, maximize simultaneous capacity, and scroll excess rows; use Single only for one agent or a pane too narrow for two useful cards.
- [ ] Separate explicit Single focus from Auto-derived presentation; persist the last Auto/Grid mode, restore it through a surface-owned exit callback, and hide Minimize for responsive Auto Single.
- [ ] Remove eager WebGL promotion from the generic renderer reveal path so the terminal's physical intersection observer is the sole WebGL visibility authority.
- [ ] Re-run the focused Agents and terminal tests.

### Task 3: Canonical inline New Tab

**Files:**
- Modify: `src/features/workbench/coreSurfaceRegistry.ts`
- Modify: `src/features/workbench/HomeSurface.tsx`
- Modify: `src/features/workbench/navigationService.ts`
- Modify: `src/features/workbench/workbenchModel.ts`
- Modify: `src/layout/workbench/WorkbenchHost.tsx`
- Modify: `src/features/workbench/surfaceIcons.ts`
- Delete: `src/features/workbench/SurfaceHomeDialog.tsx`
- Modify: `src/layout/workbench/workbench.css`
- Test: `src/layout/workbench/WorkbenchHost.test.tsx`
- Test: `src/features/workbench/navigationService.test.ts`
- Test: `src/features/workbench/workbenchModel.test.ts`

**Interfaces:**
- Produces: registered `new-tab` surface type with empty state and allow-multiple policy.
- Produces: `WorkbenchNavigationService.open_from_placeholder(surface_id, request)` that replaces the placeholder or focuses an existing singleton and discards the placeholder.

- [ ] Replace the modal expectation with tests that `+` appends and focuses a New Tab in the clicked group and renders `HomeSurface` inside its panel.
- [ ] Add navigation tests for in-place replacement and existing-singleton focus without leaving a placeholder or recently-closed entry.
- [ ] Add the canonical `new-tab` definition, exclude it from surface discovery choices, and give it a compact tab icon.
- [ ] Add an internal `discard_surface` model command sharing close/collapse behavior without adding an ephemeral placeholder to recently closed history.
- [ ] Implement `open_from_placeholder`, route Home card and Browse-all selection through it, and retain direct palette behavior when configured.
- [ ] Remove `SurfaceHomeDialog` and its modal-only CSS.
- [ ] Run the workbench unit suite and update browser E2E expectations from modal to inline content.

### Task 4: Documentation and full verification

**Files:**
- Modify: `docs/specs/2026-07-10-workbench-navigation-system.md`
- Modify: `docs/guide/workbench.md`
- Modify: `docs/guide/ui-overview.md`
- Modify: `e2e/tests/workbench-navigation.spec.ts`
- Modify: `e2e-native/tests/agent-terminal-native.test.mjs` or the nearest terminal lifecycle native spec
- Create: `e2e/screenshots/workbench-new-tab/<timestamp>/new-tab.png`

**Interfaces:**
- Produces: user-facing interaction documentation and native/browser regression evidence.

- [ ] Update the navigation spec to distinguish derived empty-pane Home from canonical user-created New Tabs.
- [ ] Update guides: default `+` opens inline New Tab; palette remains an optional preference.
- [ ] Run `npm run lint`, `npm run test`, `npm run build`, and `npm run test:e2e`.
- [ ] Run the isolated native terminal E2E without touching the live client, or rely on the PR native job if the local harness cannot coexist safely.
- [ ] Run `cargo clippy`, `cargo test`, and `cargo check` in `src-tauri`.
- [ ] Capture the inline New Tab screenshot, embed its HTTPS attachment in the PR description, verify secrets/status, commit semantically, push, and wait for green PR checks.

### Task 5: Destination-aware pane split admission

**Files:**
- Modify: `src/layout/workbench/DockviewLayoutAdapter.tsx`
- Modify: `src/layout/workbench/WorkbenchHost.tsx`
- Modify: `src/layout/workbench/WorkbenchGroupHeader.tsx`
- Modify: `src/features/workbench/useWorkbenchCommands.ts`
- Test: `src/layout/workbench/DockviewLayoutAdapter.test.tsx`
- Test: `src/layout/workbench/WorkbenchHost.test.tsx`
- Test: `e2e/tests/workbench-navigation.spec.ts`

**Interfaces:**
- Produces: shared `canSplitWorkbenchPane(bounds, position)` live-geometry predicate and exported pane minimum width/height constants.
- Consumes: live destination `group.api.boundingBox` for both overlay admission and drop commit.

- [ ] Add boundary tests at one pixel below and exactly twice each minimum, plus center and missing-bounds cases.
- [ ] Add adapter tests proving overlay and commit both reject the same impossible target while center remains valid.
- [ ] Apply explicit minimum constraints to every projected Dockview group, reject impossible `onWillShowOverlay` edge events, and repeat the predicate immediately before the drop callback.
- [ ] Gate pane menu and keyboard split actions with the same live predicate so non-pointer paths cannot bypass admission.
- [ ] Add browser E2E for a nested narrow destination: no edge preview, no topology mutation, center move still works; retain the existing viable exact-half preview assertion.
- [ ] Run focused unit and browser tests, self-review, and commit semantically.
