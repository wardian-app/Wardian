# Workbench Drag-and-Drop Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make tab and pane dragging atomic, geometrically accurate, non-fatal, and consistent with automatic empty-pane collapse while making the visual surface chooser the default `+` action.

**Architecture:** `WorkbenchDocumentV1` owns topology and collapses empty source groups transactionally. Dockview projects that model and recovers from transient group removal without throwing. `WorkbenchHost` selects a shared surface-launcher presentation from a persisted app setting.

**Tech Stack:** React 19, TypeScript, Zustand, Dockview, Vitest/Testing Library, Playwright, Rust/Tauri settings persistence.

## Global Constraints

- Wardian's workbench document remains the only persisted layout authority; never persist Dockview JSON.
- Empty non-final panes collapse automatically; the last empty pane renders Home.
- Theme variables are required for all visual styling.
- `workbench_new_tab_action` accepts only `home` or `palette`, defaulting to `home`.
- The live production Wardian client must not be stopped or restarted during development.

---

### Task 1: Transactional empty-pane collapse

**Files:**
- Modify: `src/features/workbench/workbenchModel.ts`
- Test: `src/features/workbench/workbenchModel.test.ts`

**Interfaces:**
- Consumes: `move_surface`, `removeGroupLeaf`, `leftmostGroupId`.
- Produces: cross-group `move_surface` that removes an emptied non-final source group and collapses its parent split atomically.

- [ ] **Step 1: Write failing reducer tests.** Add cases for moving the sole source tab into an existing sibling, moving the sole tab from a nested split, moving within the same group, and preserving the final group.
- [ ] **Step 2: Run the focused tests.** Run `npm run test -- src/features/workbench/workbenchModel.test.ts`; expect the empty-source assertions to fail.
- [ ] **Step 3: Implement atomic collapse.** In the cross-group branch, after building source and target groups, remove the source leaf and group record only when `sourceSurfaceIds.length === 0` and more than one group exists. Keep the target active and reject impossible tree removal.
- [ ] **Step 4: Re-run the focused tests.** Expect all workbench model tests to pass.

### Task 2: Non-fatal Dockview projection and accurate drop ownership

**Files:**
- Modify: `src/layout/workbench/DockviewLayoutAdapter.tsx`
- Modify: `src/layout/workbench/workbench.css`
- Test: `src/layout/workbench/DockviewLayoutAdapter.test.tsx`

**Interfaces:**
- Consumes: canonical `document`, Dockview `onWillDrop`, `onDidMovePanel`, group bounding boxes.
- Produces: `reconcileDockview` result with recoverable missing groups and one canonical command per accepted drag.

- [ ] **Step 1: Add failing adapter tests.** Simulate Dockview removing an empty just-created group, assert no throw and a bounded reconcile request; assert a move from the final tab does not emit a second stale command.
- [ ] **Step 2: Run the focused adapter tests.** Run `npm run test -- src/layout/workbench/DockviewLayoutAdapter.test.tsx`; expect recovery assertions to fail.
- [ ] **Step 3: Make reconciliation total.** Replace the missing-group throw with a recoverable result, retry group creation once in the same projection transaction, and schedule canonical reconciliation if Dockview still omits it. Never continue panel projection against an absent group.
- [ ] **Step 4: Serialize drag feedback.** Guard move/layout callbacks through the full batch projection microtask, discard events whose group no longer exists in the current document, and reconcile rejected events once.
- [ ] **Step 5: Align previews.** Add a Wardian drop-overlay class driven by the destination group's content rectangle and edge position; ensure `top`, `bottom`, `left`, and `right` represent exact halves and `center` fills the content box.
- [ ] **Step 6: Re-run adapter tests.** Expect all adapter tests to pass with no unhandled errors.

### Task 3: Default visual chooser and persisted `+` preference

**Files:**
- Modify: `src/types/settings.ts`
- Modify: `src/store/useSettingsStore.ts`
- Modify: `src/store/useSettingsStore.test.ts`
- Modify: `src-tauri/src/utils/app_settings.rs`
- Modify: `src/features/settings/SettingsModal.tsx`
- Modify: `src/features/settings/SettingsModal.test.tsx`
- Modify: `src/layout/workbench/WorkbenchHost.tsx`
- Modify: `src/layout/workbench/WorkbenchHost.test.tsx`
- Modify: `src/views/App.tsx`

**Interfaces:**
- Produces: `WorkbenchNewTabAction = 'home' | 'palette'`, `workbenchNewTabAction`, setter, persisted `workbench_new_tab_action`, and `WorkbenchHost.new_tab_action`.

- [ ] **Step 1: Add failing settings tests.** Assert missing/invalid values normalize to `home`, `palette` round-trips, and overrides contain only non-default values.
- [ ] **Step 2: Add failing host tests.** Assert `+` opens the visual chooser in `home` mode, opens `OpenSurfaceDialog` in `palette` mode, and Quick Open remains unchanged.
- [ ] **Step 3: Implement the typed setting end to end.** Add the TypeScript and Rust fields, normalizers/defaults, Zustand state and setter, Settings select, App prop wiring, and Rust round-trip coverage.
- [ ] **Step 4: Share the visual chooser.** Extract or reuse the Home surface-choice grid in an accessible modal/popover launched by `+`; keep the singleton empty-pane Home rendering unchanged.
- [ ] **Step 5: Run focused tests.** Run the three frontend test files and `cargo test app_settings --manifest-path src-tauri/Cargo.toml`; expect all to pass.

### Task 4: Real drag regression coverage and evidence

**Files:**
- Modify: `e2e/tests/workbench-adapter-proof.spec.ts`
- Modify: `e2e/tests/workbench-navigation.spec.ts`
- Create: `e2e/screenshots/workbench-drag-drop/<timestamp>/drag-preview.png`
- Modify: `docs/guide/navigation-workbench.md`

**Interfaces:**
- Consumes: browser workbench fixture and real Dockview pointer drag.
- Produces: regression proof for reorder, split, cross-pane move, collapse, preview geometry, and fatal-error absence.

- [ ] **Step 1: Add real-pointer E2E.** Measure the source tab and destination group, drag through center and edge positions, assert overlay bounds within two CSS pixels of the expected half, release, and assert canonical group/tab order.
- [ ] **Step 2: Add sole-tab collapse E2E.** Move the last tab out of a pane and assert its group disappears, its sibling fills the released bounds, and `Fatal UI Rendering Error` never appears.
- [ ] **Step 3: Document the interaction.** Describe automatic empty-pane collapse and the new-tab action preference in the guide.
- [ ] **Step 4: Capture feature evidence.** Save a screenshot showing an edge-drop preview and resulting split under the feature-specific screenshot directory.

### Task 5: Full verification and delivery

**Files:**
- Review all changed files and PR #667.

- [ ] **Step 1: Run frontend validation.** Run `npm run lint`, `npm run test`, `npm run test:e2e`, and `npm run build`; expect zero failures.
- [ ] **Step 2: Run backend validation.** In `src-tauri`, run `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, and `cargo check`; expect zero failures.
- [ ] **Step 3: Check integrity.** Run `git diff --check`, `git status --short`, and inspect changed files for secrets or unrelated edits.
- [ ] **Step 4: Commit and push.** Use a conventional commit such as `fix(workbench): stabilize tab and pane dragging` and push `feat/navigation-workbench`.
- [ ] **Step 5: Update PR #667.** Add verification results and embed the uploaded feature screenshot using an HTTPS GitHub-hosted image.

