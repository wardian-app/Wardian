# Workbench E2E and Documentation Report

## Status

Complete for the scoped browser E2E, screenshot, guide, spec, and plan work. Production implementation was not modified, and no live Wardian client was restarted.

## Commits

| Commit | Evidence |
|---|---|
| `502f483a` | Adapted the reusable surface-opening fixture for the default visual chooser. Added captured-pane selection, **Browse all surfaces**, persisted palette preference, Quick Open, and unified roster-menu coverage. |
| `44dd76e4` | Added explicit default/home-mode proof that Quick Open remains searchable and bypasses the visual `+` chooser. |
| `a9e972a1` | Added real-pointer tab reorder, center move with sole-source collapse, edge split, stabilized overlay geometry, saved-topology assertions, fatal-error monitoring, and the feature screenshot. |

## Browser Evidence

- Launcher, preference, and unified-menu focus:
  - `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/workbench-navigation.spec.ts --project smoke --grep "opens every migrated|visual plus chooser|persisted palette|reveals roster agents"`
  - Result: 4 passed in 9.7s.
- Default visual-chooser Quick Open:
  - `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/workbench-navigation.spec.ts --project smoke --grep "opens the visual plus chooser in its captured pane and transitions Browse all to search"`
  - Result: 1 passed in 5.0s.
- Stabilized edge preview repeated three times:
  - `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/workbench-navigation.spec.ts --project smoke --grep "shows an accurate half-pane edge preview" --repeat-each=3`
  - Result: 3 passed in 10.1s with no retries or flaky classification.
- Combined real-pointer scenarios:
  - `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/workbench-navigation.spec.ts --project smoke --grep "reorders tabs with real pointer coordinates|moves a sole source tab to another pane center|shows an accurate half-pane edge preview"`
  - Result: 3 passed in 8.8s.
- `npm run lint` passed after the launcher/menu subset and after the pointer-drag subset.

The drag tests use Playwright mouse coordinates. They poll canonical tab ownership, group count and bounds, and the latest saved `WorkbenchDocumentV1`. The edge test waits for consecutive animation-frame samples of both the Dockview selection and its live content target before comparing geometry, which removed the initial transition-dependent flake.

## Screenshot

- Path: `e2e/screenshots/workbench-drag-drop/2026-07-14/edge-preview.png`
- Size: 53,870 bytes.
- State shown: Queue tab held over the right edge with a visible 50/50 content-area drop preview and surrounding pane context.
- The repository normally ignores `e2e/screenshots/`; this feature-specific file was intentionally force-added in `a9e972a1`.

## Documentation

- `docs/guide/workbench.md` now covers the default visual `+` chooser, searchable handoff, **Settings > Appearance > New tab button**, Quick Open invariance, pointer reorder/move/split, and automatic non-final pane collapse with final-pane Home behavior.
- `docs/specs/2026-07-14-workbench-drag-drop-resilience.md` is marked implemented and browser-verified with exact evidence references.
- `docs/superpowers/plans/2026-07-14-workbench-drag-drop-resilience.md` records completed implementation tasks while leaving push and PR update actions open.

## Documentation Verification

- `npm run docs:build`: passed in 9.36s. VitePress emitted its existing large-chunk advisory; the build completed successfully.
- `npm run docs:check-llms`: passed.
- `git diff --check`: passed before the final documentation commit.

## Remaining Delivery Notes

- Browser E2E intentionally makes no native PTY, Tauri IPC, or provider-runtime claim.
- PR #667 still needs the feature screenshot uploaded to an approved HTTPS host and embedded in the PR body, followed by the external push/PR update steps. Those actions were outside this scoped task.
