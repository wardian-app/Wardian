# Workbench Drag-and-Drop Resilience Implementation Plan

> **Implementation status:** Tasks 1-4 are complete. Delivery steps remain checked only when their exact command or external action has completed.

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

- [x] **Step 1: Write failing reducer tests.** Add cases for moving the sole source tab into an existing sibling, moving the sole tab from a nested split, moving within the same group, and preserving the final group.
- [x] **Step 2: Run the focused tests.** Run `npm run test -- src/features/workbench/workbenchModel.test.ts`; expect the empty-source assertions to fail.
- [x] **Step 3: Implement atomic collapse.** In the cross-group branch, after building source and target groups, remove the source leaf and group record only when `sourceSurfaceIds.length === 0` and more than one group exists. Keep the target active and reject impossible tree removal.
- [x] **Step 4: Re-run the focused tests.** Expect all workbench model tests to pass.

### Task 2: Non-fatal Dockview projection and accurate drop ownership

**Files:**
- Modify: `src/layout/workbench/DockviewLayoutAdapter.tsx`
- Modify: `src/layout/workbench/workbench.css`
- Test: `src/layout/workbench/DockviewLayoutAdapter.test.tsx`

**Interfaces:**
- Consumes: canonical `document`, Dockview `onWillDrop`, `onDidMovePanel`, group bounding boxes.
- Produces: `reconcileDockview` result with recoverable missing groups and one canonical command per accepted drag.

- [x] **Step 1: Add failing adapter tests.** Simulate Dockview removing an empty just-created group, assert no throw and a bounded reconcile request; assert a move from the final tab does not emit a second stale command.
- [x] **Step 2: Run the focused adapter tests.** Run `npm run test -- src/layout/workbench/DockviewLayoutAdapter.test.tsx`; expect recovery assertions to fail.
- [x] **Step 3: Make reconciliation total.** Replace the missing-group throw with a recoverable result, retry group creation once in the same projection transaction, and schedule canonical reconciliation if Dockview still omits it. Never continue panel projection against an absent group.
- [x] **Step 4: Serialize drag feedback.** Guard move/layout callbacks through the full batch projection microtask, discard events whose group no longer exists in the current document, and reconcile rejected events once.
- [x] **Step 5: Align previews.** Use Dockview's public drop-overlay model with the destination content rectangle; ensure `top`, `bottom`, `left`, and `right` represent exact halves and `center` fills the content box.
- [x] **Step 6: Re-run adapter tests.** Expect all adapter tests to pass with no unhandled errors.

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

- [x] **Step 1: Add failing settings tests.** Assert missing/invalid values normalize to `home`, `palette` round-trips, and overrides contain only non-default values.
- [x] **Step 2: Add failing host tests.** Assert `+` opens the visual chooser in `home` mode, opens `OpenSurfaceDialog` in `palette` mode, and Quick Open remains unchanged.
- [x] **Step 3: Implement the typed setting end to end.** Add the TypeScript and Rust fields, normalizers/defaults, Zustand state and setter, Settings select, App prop wiring, and Rust round-trip coverage.
- [x] **Step 4: Share the visual chooser.** Extract or reuse the Home surface-choice grid in an accessible modal/popover launched by `+`; keep the singleton empty-pane Home rendering unchanged.
- [x] **Step 5: Run focused tests.** Run the three frontend test files and `cargo test app_settings --manifest-path src-tauri/Cargo.toml`; expect all to pass.

### Task 4: Real drag regression coverage and evidence

**Files:**
- Modify: `e2e/tests/workbench-navigation.spec.ts`
- Modify: `e2e/fixtures/workbench.ts`
- Create: `e2e/screenshots/workbench-drag-drop/2026-07-14/edge-preview.png`
- Modify: `docs/guide/workbench.md`

**Interfaces:**
- Consumes: browser workbench fixture and real Dockview pointer drag.
- Produces: regression proof for reorder, split, cross-pane move, collapse, preview geometry, and fatal-error absence.

- [x] **Step 1: Add real-pointer E2E.** Measure the source tab and destination content target, drag through center and edge positions, stabilize the live rectangles, assert a contained 50% overlay, release, and assert canonical group/tab order and persisted topology.
- [x] **Step 2: Add sole-tab collapse E2E.** Move the last tab out of a pane and assert its group disappears, its sibling fills the released bounds, and no page or fatal projection error occurs.
- [x] **Step 3: Document the interaction.** Describe automatic empty-pane collapse and the new-tab action preference in the guide.
- [x] **Step 4: Capture feature evidence.** Save a screenshot showing the 50/50 edge-drop preview under the feature-specific screenshot directory.

### Task 5: Full verification and delivery

**Files:**
- Review all changed files and PR #667.

- [x] **Step 1: Run frontend validation.** `npm run lint`, `npm run test` (1,908 passed, 1 skipped), `npm run test:e2e` (97 passed, 18 native-only skipped), and `npm run build` completed successfully.
- [x] **Step 2: Run backend validation.** `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (1,161 passed across the app and integration suites), and `cargo check` completed successfully. Repository-wide `cargo fmt --check` still reports pre-existing formatting drift in unrelated Rust modules; direct `rustfmt --edition 2021 --check src-tauri/src/utils/app_settings.rs` passes for the Rust file changed by this branch.
- [x] **Step 3: Check integrity.** Run `git diff --check`, `git status --short`, and inspect changed files for secrets or unrelated edits.
- [x] **Step 4: Commit and push.** The reviewed conventional commits were pushed to `feat/navigation-workbench`.
- [x] **Step 5: Update PR #667.** The PR records final verification and embeds the feature-specific edge-preview screenshot from the branch using an HTTPS GitHub-hosted image.
