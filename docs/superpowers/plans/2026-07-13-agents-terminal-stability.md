# Agents Terminal Stability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove Agents-grid terminal flicker and slow backscroll while restoring Codex composer theming and watchlist reveal behavior.

**Architecture:** The grid will virtualize only the expensive terminal presentation lifecycle while retaining stable cards and state. The terminal broker adapter will keep canonical PTY/parser geometry separate from local renderer geometry and normalize restored snapshots through the existing provider capability layer. Watchlist navigation will expose an explicit reveal callback distinct from tab-opening actions.

**Tech Stack:** React 19, TypeScript, xterm.js, Tauri IPC terminal broker, Vitest, Playwright.

## Global Constraints

- Do not stop or replace the user's live Wardian process.
- Rust remains authoritative for PTY state and native geometry.
- Use existing semantic theme variables and strict TypeScript types.
- Keep explicit Open and Open to Side commands available in the watchlist context menu.

---

### Task 1: Viewport-scoped Agents renderers

**Files:**
- Modify: `src/views/AgentsOverviewView.tsx`
- Test: `src/views/AgentsOverviewView.test.tsx`

**Interfaces:**
- Consumes: `visibleAgentIds` from `useAgentsOverviewLayout`.
- Produces: a viewport-presence set used to pass `mounted` only to nearby terminal cards.

- [x] **Step 1: Write the failing tests**

Add an IntersectionObserver harness that reports one card as intersecting and
assert only that card receives a mounted, visible `AgentTerminal`. Add a second
test with 30 agents and assert off-viewport cards stay suspended.

- [x] **Step 2: Run tests to verify failure**

Run: `npm run test -- src/views/AgentsOverviewView.test.tsx`

Expected: FAIL because all layout-visible cards currently mount terminal renderers.

- [x] **Step 3: Implement viewport lifecycle tracking**

Observe stable card elements under the Agents scroll container with a small
vertical root margin. Use the intersection set in addition to logical surface
visibility when choosing `visibility` and `renderState`; fall back to logical
visibility when IntersectionObserver is unavailable.

- [x] **Step 4: Run tests to verify pass**

Run: `npm run test -- src/views/AgentsOverviewView.test.tsx`

Expected: PASS with only intersecting cards mounted.

### Task 2: Preserve terminal fixes through broker restore

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`

**Interfaces:**
- Consumes: `normalizeCodexComposerBackgroundForTheme` and broker snapshots/events.
- Produces: provider-normalized restored frames and parser-only canonical geometry.

- [x] **Step 1: Write failing broker tests**

Register a light-theme Codex presentation whose initial snapshot includes
`ESC[48;2;41;41;41m`, then assert the browser renderer receives
`ESC[48;2;242;240;235m`. Emit broker geometry that differs from the fitted host
and assert the parser changes but the browser renderer remains locally fitted.

- [x] **Step 2: Run tests to verify failure**

Run: `npm run test -- src/features/terminal/AgentTerminal.test.tsx`

Expected: FAIL because snapshot writes are raw and canonical geometry resizes xterm.

- [x] **Step 3: Implement the minimal broker repair**

Normalize decoded snapshot state with the current terminal capability context.
Split canonical geometry application so it resizes only the headless parser;
local xterm fitting remains controlled by `fitTerminalToContainer`. Integrate
the forced-fit protection from PR #660 so preserved row DOM cannot trigger a
resize cascade after remount.

- [x] **Step 4: Run tests to verify pass**

Run: `npm run test -- src/features/terminal/AgentTerminal.test.tsx`

Expected: PASS, including existing composer and broker ownership tests.

### Task 3: Restore watchlist reveal semantics

**Files:**
- Modify: `src/layout/watchlist/AgentWatchlist.tsx`
- Modify: `src/views/App.tsx`
- Test: `src/layout/watchlist/AgentWatchlist.test.tsx`
- Test: `src/views/App.test.tsx`

**Interfaces:**
- Produces: `onRevealAgent?: (agentId: string) => void`.
- Consumes: current active Agents surface state and roster selection controller.

- [x] **Step 1: Update tests to the intended behavior**

Assert double-click and Enter call `onRevealAgent`, select the agent, focus the
existing Agents surface, update its focused agent when in singleton mode, and do
not create an Agent Session tab. Keep context-menu Open/Open to Side assertions.

- [x] **Step 2: Run tests to verify failure**

Run: `npm run test -- src/layout/watchlist/AgentWatchlist.test.tsx src/views/App.test.tsx`

Expected: FAIL because both gestures currently call `onOpenAgent`.

- [x] **Step 3: Implement reveal without tab creation**

Add the reveal callback to the watchlist. In App, focus the most recent Agents
surface, update its focused agent state, preserve its current mode, select the
agent, then scroll the card into view after layout activation.

- [x] **Step 4: Run tests to verify pass**

Run: `npm run test -- src/layout/watchlist/AgentWatchlist.test.tsx src/views/App.test.tsx`

Expected: PASS and explicit context actions still open tabs.

### Task 4: Integrated verification and evidence

**Files:**
- Modify: `e2e/tests/workbench-navigation.spec.ts`
- Update: `docs/superpowers/plans/2026-07-13-agents-terminal-stability.md`

**Interfaces:**
- Consumes: completed Tasks 1-3.
- Produces: browser regression coverage and recorded validation evidence.

- [x] **Step 1: Add integrated regression coverage**

Use a 30-agent component test to assert only viewport-near terminal hosts mount,
the active set never exceeds the renderer budget, and the next near card takes a
released slot. Keep browser coverage at the lowest meaningful layer for Agents
layout, watchlist navigation, and workbench group recovery.

- [x] **Step 2: Run focused validation**

Run: `npm run test -- src/views/AgentsOverviewView.test.tsx src/features/terminal/AgentTerminal.test.tsx src/layout/watchlist/AgentWatchlist.test.tsx src/views/App.test.tsx`

Run: `npm run test:e2e -- e2e/tests/workbench-overview.spec.ts`

Expected: all focused unit and browser tests PASS.

- [x] **Step 3: Run the project frontend gates**

Run: `npm run lint`

Run: `npm run test`

Run: `npm run build`

Expected: all commands PASS.

- [x] **Step 4: Run backend gates if broker/backend files changed**

Run from `src-tauri`: `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`, and `cargo check --all-features`.

Expected: all commands PASS; otherwise document a pre-existing failure precisely.

## Validation Evidence

- `npm run lint`: passed.
- `npm run test`: 1,870 passed, 1 skipped.
- `npm run build`: passed.
- `npm run test:e2e`: 91 passed, 18 native-only scenarios skipped.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-features`: 1,154 unit tests plus all integration tests passed.
- `cargo check --all-features`: passed.
- `npm run docs:check-llms` and `npm run docs:build`: passed.
- Changed Rust files pass `rustfmt --check`; repository-wide `cargo fmt --check`
  remains blocked by pre-existing formatting drift in unrelated modules.
