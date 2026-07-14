# Drag Resilience Task 3 Report

## Status

Complete. The workbench `+` launcher now defaults to a modal visual surface chooser, supports the persisted searchable-list preference, preserves searchable keyboard Quick Open behavior, and passes the resolved preference from app settings into the already-booted workbench.

## Commit

- Implementation: `925cb97a802b1017c3cbdaa63ceed3dffdb9ff1d` (`feat(workbench): add configurable new tab launcher`)

## Files changed

- `src/types/settings.ts`
- `src/store/useSettingsStore.ts`
- `src/store/useSettingsStore.test.ts`
- `src-tauri/src/utils/app_settings.rs`
- `src/features/settings/SettingsModal.tsx`
- `src/features/settings/SettingsModal.test.tsx`
- `src/features/workbench/SurfaceHomeDialog.tsx`
- `src/layout/workbench/WorkbenchHost.tsx`
- `src/layout/workbench/WorkbenchHost.test.tsx`
- `src/layout/workbench/workbench.css`
- `src/views/App.tsx`
- `src/views/App.test.tsx`

## RED evidence

- Store tests: 2 expected failures because `normalizeWorkbenchNewTabAction` and `setWorkbenchNewTabAction` were absent.
- Settings UI test: expected failure because no control labeled `New tab button` existed.
- Host tests: the new default visual chooser, Browse-all handoff, and visual Escape/focus cases failed because `+` still opened `Open Surface` directly and no `Choose a surface` dialog existed.
- Rust RED execution was attempted before implementation, but Cargo waited on the shared build-directory lock and timed out after 124 seconds without reaching the assertions. The requested work continued on the frontend; the completed Rust suite later passed.

## GREEN evidence and test results

- `npm run test -- src/store/useSettingsStore.test.ts src/features/settings/SettingsModal.test.tsx`: 41 passed.
- New launcher-focused host selection (`workbench new tab|visual chooser|searchable list|keyboard Quick Open`): 9 passed across the changed store/settings/host files, 44 unrelated tests skipped by the name filter.
- `npm run test -- src/views/App.test.tsx`: 76 passed.
- `cargo test app_settings --manifest-path src-tauri/Cargo.toml`: 11 passed, 0 failed.
- `npm run lint`: passed.
- `npm run build`: passed; Vite emitted its existing large-chunk advisory.
- `git diff --check`: passed before commit.
- `rustfmt --edition 2021 src-tauri/src/utils/app_settings.rs`: applied cleanly to the scoped Rust file.

## Self-review

- `home` is the frontend and Rust default; missing, blank, and invalid frontend values normalize to `home`.
- Only `palette` survives sparse override normalization and persistence; resetting to `home` removes the override.
- The modal reuses `HomeSurface` and the existing registry rather than introducing another catalog.
- The `+` callback captures its group before opening; navigation is invoked only after a visual card is selected.
- Browse-all changes presentation without changing the captured group.
- Escape, backdrop close, initial focus, and focus restoration are covered.
- Embedded empty-pane Home behavior remains unchanged, while keyboard Quick Open and Open Surface commands continue to resolve to the searchable presentation.
- Modal colors use Wardian theme variables only.
- App boot is not gated on settings; `home` is passed until settings finish loading, then the resolved store value is passed.
- No drag/drop, model, E2E, or approved docs content was changed by this task.

## Concerns / known unrelated failures

- Running the entire `src/layout/workbench/WorkbenchHost.test.tsx` file includes one pre-existing Task 1 failure in `translates edge drops into one canonical split-and-move transaction`: the stale assertion expects a split root, while canonical sole-source-pane collapse now returns the surviving `{ kind: "group", group_id: "group-new-1" }`. Per orchestration direction, this drag/drop expectation was left untouched for the queued canonical-collapse integration task.
- Repo-wide `cargo fmt --check` is blocked by pre-existing formatting differences in unrelated Rust files such as `src-tauri/src/commands/agent.rs`, `src-tauri/src/commands/git.rs`, and `src-tauri/src/control.rs`. The scoped `app_settings.rs` file was formatted directly.
- The initial focused Cargo RED run was delayed by another process holding the shared build-directory lock; a later focused run completed successfully.
- Unrelated whitespace edits in the two existing drag-resilience docs remain unstaged and uncommitted.
- Screenshot/PR evidence was not produced because the Task 3 brief explicitly excluded E2E/docs work; the eventual frontend PR still needs its required embedded hosted screenshot.

## Review hardening follow-up

Reviewer findings against `925cb97a802b1017c3cbdaa63ceed3dffdb9ff1d` were fixed with a focused RED/GREEN cycle:

- RED: Quick Open and Open Surface left an already-open Home chooser visible instead of switching it to the searchable presentation.
- RED: Tab did not wrap inside the Home dialog; Ctrl/Cmd+W and F6 could reach workbench command handling behind the modal.
- RED: a direct Quick Open session after a prior `+` session was refocused by stale host-owned state.
- RED: new inactive-pane coverage was added to prove both visual selection and Browse-all palette selection preserve the clicked pane's group id.
- GREEN: host-owned searchable-launch requests now switch presentation even when `launcher_open` is already true, while preserving the captured group for an existing `+` session.
- GREEN: Home traps Tab/Shift+Tab and the workbench command listener suppresses non-launcher commands while a launcher modal is open.
- GREEN: `OpenSurfaceDialog` owns palette focus restoration, optionally using the current `+` session's explicit return target; host focus ownership is cleared on close and before direct searchable launches.
- GREEN: 13 launcher-focused host tests passed (including Ctrl and Command Quick Open variants), 8 `OpenSurfaceDialog` tests passed, `npm run lint` passed, and `git diff --check` passed.
- Known unrelated failure remains unchanged: the stale edge-drop split-root expectation in the full host file still fails after Task 1's canonical sole-source-pane collapse. The broader command-router test also encounters the same pre-existing collapse assumption for `group-2`; neither model/drag assertion was changed in this follow-up.
