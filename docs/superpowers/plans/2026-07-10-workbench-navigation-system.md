# Workbench Navigation System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Wardian's fixed global main-view navigation with a fast, restorable workbench of pane-local tabs and splits, responsive multi-agent surfaces, and safe simultaneous desktop/remote terminal presentations.

**Architecture:** Wardian owns a normalized `WorkbenchDocumentV1`, surface registry, command/navigation service, versioned Rust persistence, and terminal-session broker. Dockview 7.0.2 is accepted only through a Phase 0 adapter proof and never owns durable state. Existing domain stores remain the resource source of truth; surface instances own presentation state. One Rust broker owns PTY geometry/input leases and provides bounded ordered snapshots/events to independent xterm renderers.

**Tech Stack:** React 19.2.4 (lockfile resolved), TypeScript 5.8, Zustand 5, `dockview-react` 7.0.2 candidate, xterm 6, Rust/Tauri 2, Tokio, portable-pty, vt100, Serde, Vitest/React Testing Library, Playwright browser E2E, Tauri/WebDriver native E2E.

**Spec:** `docs/specs/2026-07-10-workbench-navigation-system.md` — read it in full before starting any task.

**Tracking:** GitHub epic #513. Child issues #514-#523 are rebaselined to the task groups below; every commit and the final PR remain on `feat/navigation-workbench`.

## Global Constraints

- Work only in `feat/navigation-workbench`; never commit to `main`. Preserve unrelated worktrees and user changes.
- `src/types/index.ts` owns frontend DTO/shared types. IPC fields are `snake_case` in Rust and TypeScript. Do not use `any`.
- The left icon rail remains an auxiliary-tool selector. Never make it the replacement global surface launcher.
- The right roster keeps target selection separate from Open/Open to Side navigation.
- The workbench model and Rust DTO are canonical. Dockview is a rendering/drag adapter; never persist `api.toJSON()` or other Dockview-private state.
- `<wardian-home>/settings/workbench.json` and `workbench.backup.json` are the only durable workbench/shell-layout files after migration. Never serialize credentials, runtime truth, PTY contents, leases, or arbitrary unbounded state.
- Exactly one terminal presentation owns input and canonical geometry. Passive mount/focus/restore never transfers ownership; mirrors never resize the PTY.
- Structured prompt delivery remains outside the terminal-keystroke lease. Do not accidentally gate Queue, Chat, Commands, mailbox, or workflow delivery on a visible terminal.
- Keep browser and native test boundaries honest: browser E2E proves UI/model behavior; native E2E proves Tauri IPC, disk persistence, PTY geometry, and desktop/remote lease behavior.
- Use an explicit isolated `WARDIAN_HOME` for every performance/native run. Measurement scripts must fail closed when none is supplied.
- UI uses Wardian semantic theme variables/classes. Tab/split/launcher behavior is keyboard-equivalent and uses correct ARIA roles.
- `settingsOpen`, transient drag/hover/DOM focus, group zoom, terminal renderer state, and recomputable domain data are not persisted.
- Every destructive workbench command runs close/dirty guards transactionally. A cancel or failed save leaves the model and persisted revision unchanged.
- Keep one canonical writer throughout migration. Legacy navigation/terminal commands are temporary adapters with explicit deletion tasks.
- Commit only after the task's focused tests pass. Use Conventional Commits and keep commits atomic.
- Before PR: `npm run lint`, `npm run test`, `npm run build`, `npm run docs:build`, `npm run test:e2e`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace -- --test-threads=1`, `cargo check --workspace`, targeted then full native E2E, screenshot evidence, secrets scan, and clean intended git status.

---

## Pre-execution — Rebaseline live tracking

### Task 0: Rewrite epic #513 and children #514-#523 around the approved workbench

**Files:** No repository file changes.

**Issue titles:**

| Issue | Title |
|---|---|
| #513 | `[Epic] Workbench navigation: tabs, splits, surfaces, and terminal continuity` |
| #514 | `Evaluate the workbench layout adapter and establish performance gates` |
| #515 | `Implement the Wardian workbench model, surface registry, launcher, and persistence` |
| #516 | `Migrate core views to surfaces and remove fixed titlebar navigation` |
| #517 | `Implement pane-local tabs, splits, movement, zoom, and keyboard access` |
| #518 | `Route auxiliary tools and the agent roster through NavigationService` |
| #519 | `Implement the terminal presentation broker and ownership leases` |
| #520 | `Implement responsive Agents Overview Auto, Grid, and Single modes` |
| #521 | `Enforce surface lifecycle policies and renderer budgets` |
| #522 | `Implement workbench recovery, reset, placeholders, and safe mode` |
| #523 | `Cut over workbench navigation, migrate tests/docs, and verify release readiness` |

- [x] **Step 1: Rewrite #513** to link the approved spec and this plan; state explicitly that Sites/Cohorts are rejected as foundational navigation concepts and Teams/watchlists remain independent.
- [x] **Step 2: Rewrite every child body.** Begin with `Parent epic: #513` and `Rebaselined on 2026-07-10`; remove superseded Site/Cohort acceptance criteria; link the corresponding plan tasks and copy concrete testable outcomes.
- [x] **Step 3: Preserve labels and open state.** Do not close/recreate issues for bookkeeping and do not claim completion before code/tests land.
- [x] **Step 4: Re-read all 11 live issues with `gh issue view`** and verify titles, spec/plan links, dependency order, and no stale `HabitatLayout`, Site, Cohort, or Perspective implementation requirement.

## Phase 0 — Prove the adapter and freeze contracts

### Task 1: Evaluate Dockview 7.0.2 through a disposable adapter proof

**Files:**
- Modify: `package.json`
- Modify: `package-lock.json`
- Modify: `vite.config.ts` (only if the bundle needs a dedicated `vendor-workbench` chunk)
- Create: `src/layout/workbench/proof/DockviewEvaluationHarness.tsx`
- Create: `src/layout/workbench/proof/DockviewEvaluationHarness.test.tsx`
- Create: `src/layout/workbench/proof/dockviewEvaluation.css`
- Create: `e2e/tests/workbench-adapter-proof.spec.ts`
- Create: `scripts/measure-workbench-performance.mjs`
- Create: `docs/research/workbench-navigation/dockview-evaluation.md`
- Create: `docs/research/workbench-navigation/dockview-baseline.json`
- Modify: `src/views/App.tsx` (temporary dev/test-only `?workbench-proof=1` route; delete or promote before this task commits)

**Interfaces:**
- The proof renders 20 synthetic tabs in four groups, an xterm owner plus three independent mirrors, and real `GraphView`/`GardenView` wrappers.
- The measurement script requires `WARDIAN_HOME` and exits non-zero if it is absent or resolves to the production Wardian home.
- The evaluation document records package version, MIT license, 3,300,226-byte unpacked package size, React 19 peer compatibility, production bundle delta, React commit/switch/drag timings, mounted renderer/WebGL counts, accessibility findings, maintenance/release evidence, and an explicit `Decision: Promote | Reject`.

- [x] **Step 1: Write the failing component test.** Assert the proof creates four independently addressable groups, 20 tabs, moves a tab without remounting its keyed child, zooms/unzooms without changing the serialized proof model, and restores from the proof model rather than Dockview JSON.
- [x] **Step 2: Run** `npm run test -- src/layout/workbench/proof/DockviewEvaluationHarness.test.tsx`. **Expected:** FAIL because the harness and dependency do not exist.
- [x] **Step 3: Install the exact React candidate** with `npm install --save-exact dockview-react@7.0.2`. Do not substitute the vanilla `dockview` package or accept the experimental dist-tag.
- [x] **Step 4: Implement the smallest proof.** Import `DockviewReact` from `dockview-react` and `dockview-react/dist/styles/dockview.css`, wrap it in Wardian semantic variables, and keep a separate plain proof model. Do not call or persist Dockview serialization.
- [x] **Step 5: Add the browser proof.** Use real drag/drop and keyboard equivalents; assert no console errors, correct ARIA tab roles, stable component instance counters, one owner/three distinct xterm hosts, and Graph/Garden hide/show.
- [x] **Step 6: Make the performance script fail closed.** It must reject missing, empty, profile-root, or non-isolated `WARDIAN_HOME`; seed only its explicit temp home.
- [x] **Step 7: Run the proof in an explicit temp home.** POSIX:

  ```bash
  WARDIAN_HOME="$(mktemp -d)" npm run test:e2e -- workbench-adapter-proof.spec.ts
  WARDIAN_HOME="$(mktemp -d)" node scripts/measure-workbench-performance.mjs
  ```

  PowerShell:

  ```powershell
  $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("wardian-workbench-proof-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Path $tempRoot | Out-Null
  $env:WARDIAN_HOME = $tempRoot
  npm run test:e2e -- workbench-adapter-proof.spec.ts
  node scripts/measure-workbench-performance.mjs
  ```

  **Expected:** 20 tabs/four groups pass; keyboard and drag paths work; no renderer host is stolen; JSON baseline is emitted.
- [x] **Step 8: Record the promote/reject decision.** Promote only if React 19.2.4 is clean, Wardian can fully drive layout from its model, no private serialization/lifecycle dependency exists, keyboard/ARIA gaps are fixable in the adapter, and bundle/performance deltas are accepted in the document. If rejected, remove Dockview/package changes and revise Task 6 to a Wardian/custom adapter before continuing.
- [x] **Step 9: Promote or remove the proof route.** Never leave a second prototype navigation path. Keep the measurement scenario only if it drives the production adapter contract.
- [x] **Step 10: Re-run** the focused Vitest, browser proof, `npm run lint`, and `npm run build`. **Expected:** PASS.
- [x] **Step 11: Commit** `chore(workbench): evaluate dockview layout adapter`.

## Phase 1 — Canonical workbench and persistence

### Task 2: Implement `WorkbenchDocumentV1` and the pure command model

**Files:**
- Modify: `src/types/index.ts`
- Create: `src/features/workbench/workbenchModel.ts`
- Create: `src/features/workbench/workbenchModel.test.ts`
- Create: `src/features/workbench/workbenchTestUtils.ts`
- Create: `crates/wardian-core/tests/fixtures/workbench-v1.json`

**Interfaces:**

```ts
export type WorkbenchCommand =
  | { type: "open_surface"; surface: WorkbenchSurfaceV1; group_id?: string; index?: number }
  | { type: "focus_surface"; surface_id: string }
  | { type: "close_surface"; surface_id: string }
  | { type: "reopen_closed_surface" }
  | { type: "split_group"; group_id: string; new_group_id: string; node_id: string; direction: "horizontal" | "vertical"; placement: "before" | "after" }
  | { type: "move_surface"; surface_id: string; group_id: string; index: number }
  | { type: "set_active_surface"; group_id: string; surface_id: string | null }
  | { type: "set_split_ratio"; node_id: string; ratio: number }
  | { type: "close_group"; group_id: string }
  | { type: "join_group"; source_group_id: string; target_group_id: string }
  | { type: "update_surface_state"; surface_id: string; state_schema_version: number; state: unknown }
  | { type: "update_shell"; patch: Partial<WorkbenchShellV1> };

export function createDefaultWorkbenchDocument(): WorkbenchDocumentV1;
export function validateWorkbenchDocument(value: unknown): WorkbenchValidationResult;
export function applyWorkbenchCommand(document: WorkbenchDocumentV1, command: WorkbenchCommand): WorkbenchCommandResult;
```

- [x] **Step 1: Add exact V1 DTO types** from the spec to `src/types/index.ts`, including node/group/surface/closed/shell records and validation/result types. All properties remain `snake_case`.
- [x] **Step 2: Write failing tests** for the default empty group/Home derivation, open/focus, close/reopen (20-entry cap), split/move/join/close-group behavior, ratio clamp/rejection, shell update allowlist, duplicate/missing references, cycles, 64-node depth, state/document byte limits, and unknown opaque state preservation.
- [x] **Step 3: Run** `npm run test -- src/features/workbench/workbenchModel.test.ts`. **Expected:** FAIL on missing implementation.
- [x] **Step 4: Implement validation and immutable commands.** Validate both pre-state and post-state; return the original object on failure; never silently normalize corrupt state.
- [x] **Step 5: Add deterministic randomized command sequences** (fixed PRNG seed, at least 10,000 operations across several seeds). After every accepted operation assert unique IDs, one tree reference per group, one tab reference per open surface, valid active IDs, no cycles, bounded ratios, and no lost surfaces outside `recently_closed`.
- [x] **Step 6: Add the shared JSON fixture** and a frontend contract test that parses it into the exact V1 DTO.
- [x] **Step 7: Run** the focused test and `npm run lint`. **Expected:** PASS with zero `any`.
- [x] **Step 8: Commit** `feat(workbench): add canonical workbench model`.

### Task 3: Add the surface registry, navigation service, and canonical store

**Files:**
- Modify: `src/types/index.ts`
- Create: `src/features/workbench/surfaceRegistry.ts`
- Create: `src/features/workbench/surfaceRegistry.test.ts`
- Create: `src/features/workbench/navigationService.ts`
- Create: `src/features/workbench/navigationService.test.ts`
- Create: `src/features/workbench/useWorkbenchStore.ts`
- Create: `src/features/workbench/useWorkbenchStore.test.ts`
- Create: `src/features/workbench/workbenchSelectors.ts`

**Interfaces:**

```ts
export type SurfaceDefinition<TState extends SurfaceState = SurfaceState> = {
  type: SurfaceType;
  render_policy: SurfaceRenderPolicy;
  open_policy: SurfaceOpenPolicy;
  runtime_policy: SurfaceRuntimePolicy;
  close_policy: SurfaceClosePolicy;
  state_schema_version: number;
  max_state_bytes: number;
  resource_key?: (request: OpenSurfaceRequest) => string | undefined;
  resolve_existing?: (request: OpenSurfaceRequest, candidates: WorkbenchSurfaceV1[]) => string | undefined;
  default_state: () => TState;
  serialize_state: (state: TState) => unknown;
  restore_state: (value: unknown, version: number) => SurfaceRestoreResult<TState>;
  can_close?: (surface: WorkbenchSurfaceV1) => Promise<CloseDecision> | CloseDecision;
  commands: SurfaceCommandDefinition[];
};

export interface WorkbenchNavigationService {
  open(request: OpenSurfaceRequest): string;
  open_to_side(request: OpenSurfaceRequest, direction?: "horizontal" | "vertical"): string;
  focus(surface_id: string): void;
  close(surface_id: string): Promise<CloseDecision>;
  close_group(group_id: string): Promise<CloseDecision>;
  reset_workbench(): Promise<CloseDecision>;
}
```

- [x] **Step 1: Write failing registry tests** for duplicate type rejection, serializer byte bounds, state-version restore, unknown placeholders, open-policy resolution, resource focus, explicit duplicates, commands, badges, and dirty close guards.
- [x] **Step 2: Write failing navigation/store tests** for one canonical document writer, transactional close/reset guards, active group/tab selection, runtime-only `zoomed_group_id`, durable revision/pending flags, and deterministic ID injection.
- [x] **Step 3: Run** the three focused test files. **Expected:** FAIL on missing modules.
- [x] **Step 4: Implement a registry with explicit registration order** and no React/layout-library dependency. Unknown surface records produce a `missing_surface` placeholder definition while retaining inert state.
- [x] **Step 5: Implement navigation over `applyWorkbenchCommand`.** A normal resource open focuses the registry-selected candidate; only `open_to_side`/explicit duplicate creates another presentation.
- [x] **Step 6: Implement the Zustand store without `persist` middleware.** Persistence is an injected service added in Task 5; group zoom and launcher visibility remain runtime-only.
- [x] **Step 7: Run** focused tests and `npm run lint`. **Expected:** PASS.
- [x] **Step 8: Commit** `feat(workbench): add surface registry and navigation service`.

### Task 4: Add typed Rust workbench validation and atomic persistence

**Files:**
- Modify: `crates/wardian-core/src/lib.rs`
- Modify: `crates/wardian-core/Cargo.toml` (add workspace `sha2` for acknowledgement hashes)
- Modify: `crates/wardian-core/src/models/mod.rs`
- Modify: `crates/wardian-core/src/paths.rs`
- Create: `crates/wardian-core/src/models/workbench.rs`
- Create: `crates/wardian-core/src/workbench.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Create: `src-tauri/src/commands/workbench.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Modify: `src-tauri/src/lib.rs`
- Reuse: `crates/wardian-core/tests/fixtures/workbench-v1.json`

**Interfaces:**

```rust
pub fn load_workbench_for_home(home: &Path) -> Result<WorkbenchLoadResult, WorkbenchIoError>;
pub fn save_workbench_for_home(home: &Path, request: WorkbenchSaveRequest) -> Result<WorkbenchSaveResult, WorkbenchIoError>;
pub fn reset_workbench_for_home(home: &Path, request: WorkbenchResetRequest) -> Result<WorkbenchResetResult, WorkbenchIoError>;

#[tauri::command]
pub async fn load_workbench_state() -> Result<WorkbenchLoadResult, String>;
#[tauri::command]
pub async fn save_workbench_state(document: WorkbenchDocumentV1, expected_revision: u64, expected_token: String, request_id: String, state: State<'_, AppState>) -> Result<WorkbenchSaveResult, String>;
#[tauri::command]
pub async fn reset_workbench_state(expected_revision: u64, expected_token: String, request_id: String, state: State<'_, AppState>) -> Result<WorkbenchResetResult, String>;
```

`WorkbenchLoadResult` is `{ source: primary|backup|default|future_schema, document, notice, durable_revision, durable_token }`. `durable_token` is an opaque backend SHA-256 token over the exact validated persisted bytes (or the backend's exact serialized default bytes); TypeScript never computes it. Save/reset return structured `saved|revision_conflict|future_schema` outcomes plus echoed `request_id`, `durable_revision`, and `durable_token`; I/O failures alone use Tauri errors.

Revision zero is the uninitialized/default base. The frontend is the revision
proposer; the backend is the CAS authority. A new save is accepted only when
`expected_revision == durable_revision`, `expected_token == durable_token`, and
`document.revision == expected_revision + 1` (within JavaScript's maximum safe
integer). The backend alone serializes and hashes the incoming DTO. A retry after
a lost acknowledgement is idempotent success when the incoming document's
backend token and revision exactly match the durable primary, even though its
expected token is old; the same revision with different bytes/token is always a
conflict. Reusing a `request_id` with different bytes is also a conflict.

- [x] **Step 1: Write Rust model tests** against the shared fixture and malformed variants: duplicate references, missing active IDs, cycles, >64-node depth, invalid ratios, >20 closed surfaces, >64 KiB surface state, >2 MiB document, and unknown surface state round-trip.
- [x] **Step 2: Write persistence tests** for primary/backup/default, first write from revision zero/token, required `document.revision == expected + 1`, expected-token match, echoed request ID, lost-response same-request idempotent retry, reused-request/different-content rejection, same-revision/different-content token conflict, validated backup rotation, corrupt primary, corrupt primary plus valid backup, future schema preservation, CAS conflicts, maximum-safe revision, concurrent saves, atomic temp cleanup, and reset preserving every non-workbench file.
- [x] **Step 3: Run** `cargo test -p wardian-core workbench -- --test-threads=1`. **Expected:** FAIL because modules are missing.
- [x] **Step 4: Implement exact Serde DTOs** and semantic validation in `wardian-core`. Add `workbench_path_for_home` and `workbench_backup_path_for_home` returning `settings/workbench*.json`.
- [x] **Step 5: Extend the atomic-file helper for acknowledged durability.** Add `write_bytes_atomic_durable`: write/sync a sibling temp, replace with `MoveFileExW(...WRITE_THROUGH)` on Windows or `rename` + parent-directory `sync_all` on Unix, and clean stale temps on load. Use an injectable file-ops/fault hook in tests.
- [x] **Step 6: Implement exact crash ordering.** First validate/serialize and durably stage the incoming primary temp. If the current primary is valid, durably stage and replace the backup with its exact bytes, then durably replace primary. Never rotate corrupt/future primary. Acknowledge only after the final durable replace. Test faults before/after each temp sync/backup replace/primary replace/parent sync and assert startup always finds old primary, new primary, or last-good backup—never an acknowledged missing revision.
- [x] **Step 7: Add one serialized `workbench_io_lock` to `AppState`** and thin Tauri wrappers. CAS compares on-disk durable revision plus opaque token while holding this lock; only Rust serializes/hashes bytes.
- [x] **Step 8: Register commands** in `src-tauri/src/lib.rs` and add command-level temp-home tests for structured outcomes.
- [x] **Step 9: Run** `cargo fmt --all -- --check`, the focused core/app tests, `cargo clippy -p wardian-core -- -D warnings`, and `cargo clippy -p Wardian -- -D warnings`. **Result:** Task 4 targeted rustfmt and every test/clippy/check gate passed; the repo-wide fmt command still reports documented pre-existing drift in unrelated HEAD files.
- [x] **Step 10: Commit** `feat(workbench): persist versioned workbench state`.

### Task 5: Connect persistence, first migration, durability status, and boot flags

**Files:**
- Create: `src/features/workbench/workbenchPersistence.ts`
- Create: `src/features/workbench/workbenchPersistence.test.ts`
- Create: `src/features/workbench/useWorkbenchPersistence.ts`
- Create: `src/features/workbench/useWorkbenchPersistence.test.tsx`
- Create: `src/features/workbench/WorkbenchConflictDialog.tsx`
- Create: `src/features/workbench/WorkbenchConflictDialog.test.tsx`
- Create: `src/config/workbenchFlags.ts`
- Create: `src/config/workbenchFlags.test.ts`
- Modify: `src/store/useLayoutStore.ts`
- Modify: `src/store/useLayoutStore.test.ts`
- Modify: `src/views/App.tsx`
- Modify: `src/views/App.test.tsx`

Executed scope corrections: the safe-mode contract required a separate thin
Rust boot-config command/registration, while removing the layout shadow writer
required moving Settings modal state and Library detail width to their owning
stores and updating `LibraryView`.

**Interfaces:**
- Developer migration flag: `VITE_WARDIAN_WORKBENCH=1`, default off until Task 19 after browser, native, and performance gates pass.
- Runtime rollback flag: Rust reads `WARDIAN_WORKBENCH_SAFE_MODE=1`; the frontend receives it in the load result/boot command and renders the one-group safe adapter without downgrading the document.
- The store separates `durable_document`/`durable_revision` from a mutable
  `working_document`. Local commands never invent independent durable history.
  When dirty and no write is active, the queue snapshots working state as
  revision `durable_revision + 1`, sends `expected_revision = durable_revision`,
  the backend-provided `expected_token`, and a fresh UUID `request_id`. Mutations
  during that write stay in the working draft. Only an acknowledgement echoing
  the request ID and the expected next revision advances the durable base/token;
  if the draft changed, the next snapshot becomes the following revision. The
  queue writes within 250 ms, ignores stale acknowledgements, and exposes
  `pending_request_id`, `pending_revision`, `durable_revision`, `durable_token`,
  and `save_error`. TypeScript never hashes/canonicalizes the document.
- A CAS conflict freezes further persistence but preserves the complete local
  working draft in memory. Wardian never structurally auto-merges workbench
  documents. The conflict dialog offers: Use Disk (explicitly discard local),
  Replace Disk (reload durable R, revalidate the local draft as R+1, then save
  against R), or Export Local JSON. Future-schema conflicts allow export only.

- [x] **Step 1: Write failing tests** for load sources/token, revision-zero first save, required R→R+1 relation, request ID/expected token payload, pending/durable indicators, coalesced mutations during an in-flight write, lost-response identical retry, stale/wrong-request acknowledgement, CAS conflict freeze, Use Disk, Replace Disk rebase without structural merge, Export Local JSON, future-schema export-only mode, unmount/shutdown flush as best effort only, and reset immediate flush.
- [x] **Step 2: Write migration tests** using real `wardian-layout` localStorage payloads: allowlist/clamp sidebar widths, terminal open/height; exclude Settings modal, Grid tracks/stacking, and Library detail width; default existing users to Agents Overview; corrupt/missing storage uses defaults.
- [x] **Step 3: Run** the focused tests. **Expected:** FAIL on missing persistence bridge.
- [x] **Step 4: Implement the invoke adapter and serialized save state machine.** Treat `future_schema` as read-only: preserve files, show notice, and never call save/reset. On ordinary CAS conflict, reload only for the explicit resolution dialog; never mutate the local draft automatically.
- [x] **Step 5: Implement one-time import.** Only when no valid workbench exists, create V1, optionally seed the same-run old `viewMode`, import the shell allowlist, persist it, then remove `wardian-layout` only after the backend acknowledgement.
- [x] **Step 6: Remove Zustand persistence for migrated shell/layout fields.** Move remaining Library-specific width and Settings modal state to their owning stores; do not leave a shadow writer.
- [x] **Step 7: Add feature/safe-mode tests** and a non-blocking durability/recovery notice host in `App` (temporary old-shell branch remains until cutover).
- [x] **Step 8: Run** focused tests, `npm run lint`, and `npm run build`. **Expected:** PASS with the flag off and on.
- [x] **Step 9: Commit** `feat(workbench): restore and durably save workbench state`.

### Task 6: Build the production workbench host, adapter, Home, and command router

**Files:**
- Create: `src/layout/workbench/WorkbenchHost.tsx`
- Create: `src/layout/workbench/WorkbenchHost.test.tsx`
- Create: `src/layout/workbench/DockviewLayoutAdapter.tsx` (or the promoted alternative selected by Task 1)
- Create: `src/layout/workbench/DockviewLayoutAdapter.test.tsx`
- Create: `src/layout/workbench/WorkbenchGroupHeader.tsx`
- Create: `src/layout/workbench/WorkbenchTab.tsx`
- Create: `src/layout/workbench/workbench.css`
- Create: `src/features/workbench/HomeSurface.tsx`
- Create: `src/features/workbench/HomeSurface.test.tsx`
- Create: `src/features/workbench/OpenSurfaceDialog.tsx`
- Create: `src/features/workbench/OpenSurfaceDialog.test.tsx`
- Create: `src/features/workbench/useWorkbenchCommands.ts`
- Create: `src/features/workbench/useWorkbenchCommands.test.tsx`
- Modify: `src/styles/App.css`
- Modify: `src/views/App.tsx`

**Required DOM contract:**
- group: `[data-testid="workbench-group"][data-active="true|false"]`
- group add button: `aria-label="Open Surface"`
- tabs: `role="tab"`, `data-surface-id`, `data-surface-type`, optional `data-resource-key`
- panels: `data-testid="surface-panel"`, `data-surface-id`, `data-surface-type`, optional `data-resource-key`
- launcher: `role="dialog"` named `Open Surface`; choices `role="option"` + `data-surface-type`

- [x] **Step 1: Write failing adapter/host tests** for model-to-layout projection, tab/group events translated to `WorkbenchCommand`, no layout-library JSON, empty-group derived Home, stable keyed panel instances, group zoom outside the document, close guards, deterministic Close/Join Group, and safe-mode one-group rendering.
- [x] **Step 2: Write failing launcher/command tests** for `+`, recent/reopen, type groups, singleton focus, explicit Open to Side, Quick Open, command palette actions, editable-element suppression, terminal shortcut precedence, and ARIA focus restoration.
- [x] **Step 3: Run** the focused files. **Expected:** FAIL on missing components.
- [x] **Step 4: Implement the production adapter.** Reconcile model deltas idempotently and suppress feedback loops with an adapter transaction token. Dockview panel IDs equal `surface_id`; group IDs are Wardian group IDs.
- [x] **Step 5: Implement Home/Open Surface and commands.** Include Agents Overview, Dashboard, Queue, Graph, Garden, Library, Workflows, and Agent Session only when a resource is supplied. File Editor/Browser appear only as disabled/reserved contributions until implemented.
- [x] **Step 6: Implement keyboard/accessibility.** Cover next/previous tab, next/previous group, split right/down, move tab, close, reopen, zoom, launcher, dock focus, keyboard-adjustable separators, roving tab focus, and drag alternatives.
- [x] **Step 7: Theme entirely through Wardian variables.** Do not ship Dockview's visual theme as product styling.
- [x] **Step 8: Mount behind `VITE_WARDIAN_WORKBENCH=1`** while leaving the old central router untouched for comparison. The workbench branch reads the same shared resource data; do not duplicate backend subscriptions.
- [x] **Step 9: Run** focused tests, `npm run lint`, `npm run build`, and the promoted adapter browser proof. **Expected:** PASS.
- [x] **Step 10: Commit** `feat(workbench): add tabbed split workbench host`.

## Phase 2 — Make terminal presentation safe before duplication

### Task 7: Implement the authoritative per-session terminal broker

**Files:**
- Modify: `crates/wardian-core/src/models/mod.rs`
- Create: `crates/wardian-core/src/models/terminal_session.rs`
- Create: `src-tauri/src/state/terminal_session/mod.rs`
- Create: `src-tauri/src/state/terminal_session/actor.rs`
- Create: `src-tauri/src/state/terminal_session/replay.rs`
- Create: `src-tauri/src/state/terminal_session/snapshot.rs`
- Create: `src-tauri/src/state/terminal_session/tests.rs`
- Modify: `src-tauri/src/state/mod.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Modify: `src/types/index.ts`

**Architecture and interfaces:**

```rust
pub struct TerminalSessionBroker {
    sessions: tokio::sync::RwLock<HashMap<String, TerminalSessionHandle>>,
}

pub struct TerminalSessionHandle {
    tx: tokio::sync::mpsc::Sender<TerminalSessionMessage>,
}

impl TerminalSessionBroker {
    pub async fn start_or_replace_runtime(
        &self,
        session_id: &str,
        runtime: TerminalRuntimeHandles,
        geometry: TerminalGeometry,
    ) -> Result<u64, TerminalBrokerError>;
    pub fn process_output_blocking(
        &self,
        session_id: &str,
        runtime_generation: u64,
        bytes: Vec<u8>,
    ) -> Result<(), TerminalBrokerError>;
    pub async fn register_presentation(
        &self,
        request: TerminalPresentationRegistration,
        identity: TerminalClientIdentity,
    ) -> Result<TerminalPresentationRegistrationResult, TerminalBrokerError>;
}
```

One bounded actor per live PTY owns parser, replay ring, presentation records, owner/pending lease, generation, epoch, geometry, and input/resize handles. Reader threads use bounded `blocking_send`; short actor control work may apply PTY backpressure but never drops/reorders bytes. The actor is the one ordered stream for output, geometry, ownership barriers, and generation changes. Do not hold a synchronous mutex across `await`.

**Feed protocol:** Feed consumers are distinct from interactive presentations.
One desktop `TerminalSessionClient` consumer per session fans events to all local
renderers; every authenticated remote socket has its own consumer. All consumers
read the same replay ring—none owns a private output queue.

```rust
pub struct TerminalEventReadRequest {
    pub session_id: String,
    pub consumer_id: String,
    pub runtime_generation: u64,
    pub after_sequence: u64, // last fully applied event
    pub max_events: u16,     // clamped to 1..=256
    pub max_bytes: u32,      // clamped to 1..=262_144
}

pub enum TerminalEventBatchStatus {
    Events,
    Gap,
    GenerationChanged,
    Terminated,
}

pub struct TerminalEventBatch {
    pub status: TerminalEventBatchStatus,
    pub runtime_generation: u64,
    pub events: Vec<TerminalBrokerEvent>,
    pub next_sequence: u64,
    pub available_from_sequence: u64,
    pub latest_sequence: u64,
    pub recovery_snapshot: Option<TerminalSnapshot>,
}
```

`after_sequence` is stateless cursor input; `next_sequence` is the last event in
the returned batch (or unchanged when empty). Gap/generation responses include a
fresh bounded snapshot whose barrier becomes the next cursor. Consumers
acknowledge applied sequences for diagnostics/lag metrics, but acknowledgement
does not control replay retention. Subscribe returns broker state and initial
snapshot; unsubscribe cancels wake-ups and pending reads.

The actor command channel capacity is 256. PTY reader threads block when full so
bytes are not dropped; control calls await capacity. On each stream advance, a
coalescer emits at most one `terminal-session-events-ready { session_id,
runtime_generation, latest_sequence }` Tauri wake-up per 16 ms. Desktop consumers
pull until caught up. Remote sockets use the same cursor after a broker broadcast
wake; broadcast lag becomes a Gap response, not socket-local buffering. Runtime
termination resolves pending calls with `Terminated`, closes consumers, and
awaits the actor for up to two seconds before a logged abort.

- [x] **Step 1: Add exact common DTOs** for registration/update, server-derived capability, broker state, generation/epoch, begin/ack activation, snapshot, replay events, geometry sequence, structured lease decisions, and lifecycle events. Mirror them in `src/types/index.ts` with `snake_case` fields.
- [x] **Step 2: Write failing actor tests** for passive registration, client capability downgrade/no escalation, separate local/remote limits, first-owner bootstrap, begin/begin and ack/ack idempotency, superseding begin, stale ack, five-second timeout rollback, disconnect during pending, owner-loss promotion by server sequence, hidden/read-only/suspended ineligibility, zero-presentation lifetime, bounded-channel backpressure, pending-call cancellation, and two-second shutdown.
- [x] **Step 3: Write failing stream tests** for subscribe/initial snapshot, cursor batching limits, per-consumer acknowledgement, unsubscribe, shared-ring independent consumers, 16 ms coalesced wake-up, 4,096-event/1 MiB replay limits, gap snapshot recovery, generation change, termination, split UTF-8 frames, stale-generation output rejection, concurrent output/geometry ordering, and generation reset.
- [x] **Step 4: Run** `cargo test -p Wardian terminal_session -- --test-threads=1`. **Expected:** FAIL on missing module.
- [x] **Step 5: Implement the actor and broker.** Use a test-injected timeout/clock. Effective interaction is derived from trusted desktop identity or authenticated remote policy; registration JSON can only request read-only downgrade.
- [x] **Step 6: Implement two-phase activation.** Freeze old/new input during pending; return snapshot/barrier; accept `ack` only for matching session/presentation/generation/epoch/activation; apply one desired geometry at commit; timeout reactivates an eligible prior owner at the new epoch.
- [x] **Step 7: Implement ordered geometry events.** Native resize and parser resize commit through the actor, followed by a geometry snapshot barrier before later bytes. Reject stale `geometry_sequence`.
- [x] **Step 8: Run** focused tests, `cargo fmt --all -- --check`, and `cargo clippy --workspace -- -D warnings`. **Expected:** PASS.
- [x] **Step 9: Commit** `feat(terminal): add authoritative session broker`.

### Task 8: Integrate PTY lifecycle, desktop IPC, and independent xterm presentations

**Files:**
- Create: `src-tauri/src/state/terminal_session/native.rs`
- Create: `src-tauri/src/commands/terminal_session.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/commands/terminal.rs`
- Modify: `src-tauri/src/commands/agent.rs`
- Modify: `src-tauri/src/manager/spawn.rs`
- Modify: `src-tauri/src/lib.rs`
- Create: `src/features/terminal/terminalSessionClient.ts`
- Create: `src/features/terminal/terminalSessionClient.test.ts`
- Create: `src/features/terminal/terminalRendererBudget.ts`
- Create: `src/features/terminal/terminalRendererBudget.test.ts`
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Modify: `src/features/terminal/AgentTerminal.test.tsx`

**New Tauri commands:**

```text
register_terminal_presentation
update_terminal_presentation
unregister_terminal_presentation
report_terminal_presentation_viewport
begin_terminal_activation
ack_terminal_activation
request_terminal_snapshot
subscribe_terminal_events
read_terminal_events
ack_terminal_events
unsubscribe_terminal_events
send_terminal_presentation_input
send_terminal_presentation_binary
resize_terminal_presentation
```

Every input/resize request includes `session_id`, `presentation_id`, `runtime_generation`, and `lease_epoch`; resize also includes `geometry_sequence`. Stale/not-owner conditions return a successful structured lease decision and current state, not a disconnecting Tauri error.

- [x] **Step 1: Write failing Rust command/lifecycle tests.** Prove only the committed owner reaches stdin/native resize; mirror viewport reports store desired geometry only; spawn uses broker canonical/deferred geometry; stale reader bytes cannot reach a replacement generation; pause/resume/clear/kill follow the spec; close-last presentation leaves runtime alive.
- [x] **Step 2: Run** the focused Rust tests. **Expected:** FAIL because commands/lifecycle hooks do not exist.
- [x] **Step 3: Move low-level resize authority behind the broker.** The actor owns the PTY master/input handles and global ConPTY resize gate. Remove `AppState.pty_sizes`; runtime spawn asks the broker before falling back to 80x24.
- [x] **Step 4: Wire lifecycle boundaries.** Spawn creates/replaces generation; reader captures it; pause marks unavailable; resume/clear replace generation and revoke lease; kill emits terminated and removes broker runtime without editing workbench files.
- [x] **Step 5: Write failing frontend tests** for two components rendering the same session simultaneously, distinct renderer hosts, exactly one desktop feed consumer/cursor, bounded multi-batch catch-up, gap/generation snapshot recovery, unsubscribe on last local presentation, passive mirror, click/keyboard activation handshake, hidden-owner resync, renderer-budget eviction/resync, stale nonfatal response, and independent local scale/pan.
- [x] **Step 6: Refactor `AgentTerminal`.** Require stable `presentationId`, `visibility`, `renderState`, and `requestedInteraction`. `TerminalSessionClient` subscribes once per local session, reacts to the coalesced wake-up, pulls cursor batches to completion, acknowledges applied events, and fans them to one renderer/link/title/viewport state per presentation. New/resumed renderers request their own snapshot barrier. Key WebGL ownership by presentation and enforce 24 xterm/12 WebGL limits.
- [x] **Step 7: Implement mirror fitting.** Normal font when it fits, scale to the accessibility floor, then pan; larger mirrors letterbox. Only owner calls the epoch-bearing resize command.
- [x] **Step 8: Keep legacy raw commands as broker adapters** for one stable release, but remove every production TypeScript caller. Structured prompt/delivery commands remain independent.
- [x] **Step 9: Run** `npm run test -- src/features/terminal/AgentTerminal.test.tsx src/features/terminal/terminalSessionClient.test.ts src/features/terminal/terminalRendererBudget.test.ts` and focused Rust tests. Then run `rg -n '"(send_input_to_agent|send_binary_input_to_agent|resize_agent_terminal)"' src`. **Expected:** no production callsite outside a named compatibility adapter/test.
- [x] **Step 10: Commit** `feat(terminal): migrate desktop presentations to broker leases`.

### Task 9: Migrate authenticated remote terminal streaming to broker protocol v2

**Files:**
- Create: `src-tauri/src/remote/terminal_stream.rs`
- Modify: `src-tauri/src/remote/mod.rs`
- Modify: `src-tauri/src/remote/gateway.rs`
- Delete after porting tests: `src-tauri/src/state/terminal_attach.rs`
- Modify: `src-tauri/src/state/mod.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Modify: `src/features/remote/remoteClient.ts`
- Modify: `src/features/remote/RemoteAgentDetailView.tsx`
- Modify: `src/features/remote/RemoteAgentDetailView.test.tsx`
- Modify: `src/features/remote/RemoteMobileApp.test.tsx`

**Protocol:** v2 open is `{ protocol_version: 2, ticket, cols, rows }`. The authenticated socket supplies session/presentation identity; client messages carry generation/epoch as applicable: report viewport, begin/ack activation, input, binary, resize, request snapshot/events, set presentation state, and detach.

- [x] **Step 1: Write failing gateway tests** for server-derived authenticated capability, v2 passive registration, explicit activation, desired-geometry-only mirror reports, desktop→remote→desktop transfer, nonfatal stale/non-owner responses, and lag-to-snapshot recovery.
- [x] **Step 2: Pin no-regression contracts:** three remote connections per agent, 64 KiB decoded input cap, two-second input timeout, 20..240 by 8..80 geometry, ticket/session expiry, device identity, rate limits, warm detach/generation cleanup, and socket backpressure. Invalid framing/oversized input remains fatal; lease disagreement does not.
- [x] **Step 3: Run** `cargo test -p Wardian remote::terminal_stream -- --test-threads=1`. **Expected:** FAIL before extraction.
- [x] **Step 4: Extract the oversized gateway terminal path** and route it exclusively through `TerminalSessionBroker`. Each socket subscribes one feed consumer, drains shared-ring cursor batches after wake-ups, acknowledges applied sequences, converts Gap/GenerationChanged into snapshot recovery, and unsubscribes on detach; it never owns a private raw-output queue.
- [x] **Step 5: Add a one-release v1 wire adapter.** Missing `protocol_version` preserves attach-means-owner behavior through a server-side activation, but owns no parser/geometry state. Remove `TerminalAttachState` after all tests move.
- [x] **Step 6: Update the remote React client** to display owner/mirror state, report viewport without resizing, explicitly activate, acknowledge snapshots, and keep the socket open on stale lease responses.
- [x] **Step 7: Run** focused Rust/React tests and the existing remote PWA browser suite. **Expected:** PASS.
- [x] **Step 8: Commit** `feat(remote): migrate terminal streaming to broker v2`.

## Phase 3 — Migrate Wardian's surfaces and cut over navigation

### Task 10: Extract shared agent and roster controllers from `App.tsx`

**Files:**
- Create: `src/features/agents/AgentResourceContext.tsx`
- Create: `src/features/agents/useAgentResourceController.ts`
- Create: `src/features/agents/useAgentResourceController.test.tsx`
- Create: `src/features/agents/RosterContext.tsx`
- Create: `src/features/agents/useRosterController.ts`
- Create: `src/features/agents/useRosterController.test.tsx`
- Create: `src/layout/AppShell.tsx`
- Modify: `src/views/App.tsx`
- Modify: `src/views/App.test.tsx`

**Interfaces:** `AgentResourceContext` owns one subscription/load path for agents, telemetry, titles, thoughts, status, and lifecycle methods. `RosterContext` owns the active watchlist/filter and global `selected_agent_ids`; it exposes `filtered_agents` without conflating targets with surface focus.

- [x] **Step 1: Write characterization tests** around existing load/listen/lifecycle behavior and roster plain/Ctrl-Cmd/Shift/empty selection semantics.
- [x] **Step 2: Extract controllers one state family at a time.** Delete each old `App` owner as its context lands; never mirror it in two stores.
- [x] **Step 3: Compose `AppShell`** from titlebar, left auxiliary region, central legacy/workbench branch, right roster, bottom terminal, and settings. `App.tsx` becomes startup/providers/composition.
- [x] **Step 4: Run** focused agent/roster/App tests. **Expected:** old flag-off UI remains behaviorally unchanged and multiple surfaces do not duplicate backend subscriptions.
- [x] **Step 5: Commit** `refactor(app): extract workbench resource controllers`.

### Task 11: Evolve Grid into container-aware Agents Overview

**Files:**
- Modify: `src/types/index.ts`
- Create: `src/features/grid/agentsOverviewLayout.ts`
- Create: `src/features/grid/agentsOverviewLayout.test.ts`
- Create: `src/features/grid/useAgentsOverviewLayout.ts`
- Create: `src/features/workbench/surfaces/AgentsOverviewSurface.tsx`
- Create: `src/features/workbench/surfaces/AgentsOverviewSurface.test.tsx`
- Rename/modify: `src/views/GridView.tsx` → `src/views/AgentsOverviewView.tsx`
- Rename/modify: `src/views/GridView.test.tsx` → `src/views/AgentsOverviewView.test.tsx`
- Modify: `src/features/grid/useGridResize.ts`
- Modify: `src/store/useLayoutStore.ts`

**State/constants:** `AgentsOverviewSurfaceState { mode: auto|grid|single, focused_agent_id, search_query, status_filter }`; terminal floor 520x280, chat floor 360x280, chrome 52, debounce 120 ms, improvement threshold 10%.

- [x] **Step 1: Write failing pure-layout tests** for candidate scoring/ties, hard-floor Auto→Single, explicit Grid scroll/no Single, explicit Single persistence, 120 ms debounce, 10% hysteresis, mixed chat/terminal floors, focused-agent fallback, stable ordering, and zero-agent state.
- [x] **Step 2: Replace `window.innerWidth` logic** with the surface container `ResizeObserver`. Auto counts the current roster/watchlist-filtered population; selected targets do not alter population.
- [x] **Step 3: Replace maximize with Single.** Single changes only this surface's presentation. Group zoom, target selection, tabs, and runtimes remain unchanged.
- [x] **Step 4: Derive stable terminal presentation IDs** as `${surface_id}:agent:${agent_id}`. Hidden cards remain registered/suspended and mode changes never activate ownership.
- [x] **Step 5: Run** focused Grid/Overview tests and `npm run lint`. **Expected:** PASS with no window breakpoint.
- [x] **Step 6: Commit** `feat(workbench): add responsive Agents Overview modes`.

### Task 12: Add Agent Session surfaces and roster Open/Open to Side routing

**Files:**
- Create: `src/features/workbench/surfaces/AgentSessionSurface.tsx`
- Create: `src/features/workbench/surfaces/AgentSessionSurface.test.tsx`
- Modify: `src/layout/watchlist/AgentWatchlist.tsx`
- Modify: `src/layout/watchlist/AgentWatchlist.test.tsx`
- Modify: `src/views/App.test.tsx`

- [x] **Step 1: Register `agent-session`** with focus-resource normal open, explicit duplicates, suspend-when-hidden, runtime-backed close-view behavior, state version/size, missing-agent placeholder, and resource key = agent/session UUID.
- [x] **Step 2: Write tests** proving normal open focuses the most recent matching presentation, Open to Side creates a new surface/group, closing the final tab never invokes kill, and owner/mirror badges follow broker state.
- [x] **Step 3: Split roster callbacks** into target-selection and `onOpenAgent` / `onOpenAgentToSide`. Double-click, Enter, explicit Open, and context menu navigate; plain/multiselect does not. Delete `scrollToAgent` as the navigation route.
- [x] **Step 4: Ensure focused agent state is surface-local** and never overwrites global command targets.
- [x] **Step 5: Run** focused surface/watchlist/App tests. **Expected:** PASS.
- [x] **Step 6: Commit** `feat(workbench): open agent sessions from the roster`.

### Task 13: Register and migrate Dashboard, Queue, Graph, Garden, Library, and Workflows

**Files:**
- Create: `src/features/workbench/surfaces/coreSurfaceDefinitions.tsx`
- Create: `src/features/workbench/surfaces/coreSurfaceDefinitions.test.tsx`
- Create: `src/features/workbench/surfaces/LibrarySurface.tsx`
- Create: `src/features/workbench/surfaces/WorkflowsSurface.tsx`
- Create: `src/features/workbench/surfaces/dirtySurfaceGuards.test.ts`
- Modify: `src/views/DashboardView.tsx`
- Modify: `src/views/QueueView.tsx`
- Modify: `src/views/GraphView.tsx`
- Modify: `src/views/GardenView.tsx`
- Modify: `src/views/LibraryView.tsx`
- Modify: `src/views/WorkflowsView.tsx`
- Modify their existing test files
- Modify: `src/store/useLibraryStore.ts`
- Modify: `src/store/useBuilderStore.ts`

- [x] **Step 1: Register exact policies:** Dashboard/Queue singleton + recreate; Graph/Garden singleton + suspend; Library/Workflows singleton + keep alive/dirty guard. Add state version/size/open commands/badges for each.
- [x] **Step 2: Route Queue/Graph/Garden agent actions** through `NavigationService` to Agent Session. Selection remains a separate roster action.
- [x] **Step 3: Replace App's `cachedCanvasViews`** with registry lifecycle. Use fake timers to prove Graph/Garden destroy heavy renderers after 30 hidden seconds and restore registered/shared state.
- [x] **Step 4: Add Library/Workflows Save/Discard/Cancel guards.** A failed save or cancel aborts close group/reset atomically. Keep their existing domain stores as resource owners.
- [x] **Step 5: Run** the focused surface/view/store tests. `rg -n "setViewMode|ViewMode"` on migrated components must return no matches.
- [x] **Step 6: Commit** `feat(workbench): migrate core Wardian surfaces`.

### Task 14: Route auxiliary objects and complete the feature-flagged surface migration

**Files:**
- Modify: `src/layout/SidebarContentPane.tsx`
- Modify: `src/layout/SidebarContentPane.test.tsx`
- Modify: `src/layout/titlebar/CustomTitleBar.tsx`
- Modify: `src/layout/titlebar/CustomTitleBar.test.tsx`
- Modify: `src/styles/App.css`
- Modify: `src/views/App.tsx`
- Modify: `src/views/App.test.tsx`

- [x] **Step 1: Replace auxiliary bridge callbacks** with typed `onOpenSurface`/NavigationService requests. Rail clicks still only select auxiliary panes.
- [x] **Step 2: Prove every old destination is reachable** from Home/`+`, Quick Open, command palette, or contextual object action before deleting the titlebar list.
- [x] **Step 3: Make the flagged titlebar correct.** With `VITE_WARDIAN_WORKBENCH=1`, omit the fixed launcher and expose only workbench commands while preserving drag/telemetry/side/window controls. With the flag off, retain `WorkspaceTabs` and `viewMode` solely as the rollback comparison path until Task 19.
- [x] **Step 4: Keep the workbench flag default off.** Safe mode and the normal flagged adapter consume the same model/registry; neither overwrites legacy state before migration acknowledgement.
- [x] **Step 5: Run** focused App/titlebar/sidebar tests, `npm run lint`, and `npm run build` in flag-off and flag-on configurations. **Expected:** both paths pass; the flagged path contains no fixed global launcher.
- [x] **Step 6: Commit** `feat(workbench): complete flagged surface migration`.

## Phase 4 — Prove migration, recovery, and runtime continuity

### Task 15: Migrate browser selectors behind the flag and freeze the 25-hit audit

**Files:**
- Create: `e2e/fixtures/workbench.ts`
- Create: `docs/research/workbench-navigation/legacy-titlebar-audit.md`
- Modify desktop navigation in:
  - `e2e/tests/agent-lifecycle.spec.ts`
  - `e2e/tests/critical-flows.spec.ts`
  - `e2e/tests/features.spec.ts`
  - `e2e/tests/garden.spec.ts`
  - `e2e/tests/graph-topology.spec.ts`
  - `e2e/tests/library-redesign.spec.ts`
  - `e2e/tests/queue-v2.spec.ts`
  - `e2e/tests/run-params.spec.ts`
  - `e2e/tests/run-view.spec.ts`
  - `e2e/tests/schedule-monitor.spec.ts`
  - `e2e/tests/workflow-builder.spec.ts`
  - `e2e/tests/workflow.spec.ts`
  - `e2e/tests/workflows.spec.ts`
- Track later-task disposition for `e2e-native/tests/terminal-geometry-sweep-native.test.mjs`, `e2e-native/tests/terminal-rendering-native.test.mjs`, `scripts/capture-doc-screenshots.mjs`, `scripts/capture-readme-demo-real.mjs`, and the performance script.

**Semantic helper contract:** `activeWorkbenchGroup(page)`, `surfaceTab(page, surface_type, resource_key?)`, `surfacePanel(...)`, and `openSurface(...)` consume the DOM contract from Task 6. Do not encode Dockview class names.

- [ ] **Step 1: Run the audited legacy regex** from `docs/research/workbench-navigation/legacy-titlebar-audit.md` against `e2e`, `e2e-native`, `scripts`, `src`, and `.github`. At commit `d53842dc`, expected count is exactly 25. Record every path as migrated, retained only for the flag-off comparison until Task 19, scheduled in Tasks 17/18/20, or intentionally unrelated (`remote-pwa`, `RemoteMobileApp`, Settings density).
- [ ] **Step 2: Write the semantic Playwright helper** with typed core surface names and role/data-attribute selectors.
- [ ] **Step 3: Replace all 13 desktop browser suites' fixed-button navigation.** Preserve remote PWA's own mobile Queue navigation. Configure these runs with `VITE_WARDIAN_WORKBENCH=1`; do not delete the flag-off product branch yet.
- [ ] **Step 4: Run** the 13 selected Playwright files with `VITE_WARDIAN_WORKBENCH=1`. **Expected:** PASS; no selected desktop suite clicks a fixed titlebar button and the audit accounts for all original hits.
- [ ] **Step 5: Commit** `test(workbench): migrate desktop surface navigation selectors`.

### Task 16: Add complete browser E2E for workbench behavior and recovery

**Files:**
- Create: `e2e/fixtures/workbenchIpcMock.ts`
- Create: `e2e/tests/workbench-navigation.spec.ts`
- Create: `e2e/tests/workbench-overview.spec.ts`
- Create: `e2e/tests/workbench-recovery.spec.ts`
- Create: `e2e/tests/workbench-screenshot.spec.ts`
- Modify: `package.json`

**Mock IPC contract:**

```ts
load_workbench_state(): Promise<{
  source: "primary" | "backup" | "default" | "future_schema";
  document: WorkbenchDocumentV1 | null;
  notice: string | null;
  durable_revision: number | null;
  durable_token: string | null;
}>;
save_workbench_state({ document, expected_revision, expected_token, request_id }: { document: WorkbenchDocumentV1; expected_revision: number; expected_token: string; request_id: string }): Promise<{
  outcome: "saved" | "revision_conflict" | "future_schema";
  durable_revision: number | null;
  durable_token: string | null;
  request_id: string;
}>;
reset_workbench_state({ expected_revision, expected_token, request_id }: { expected_revision: number; expected_token: string; request_id: string }): Promise<{
  outcome: "saved" | "revision_conflict" | "future_schema";
  durable_revision: number | null;
  durable_token: string | null;
  request_id: string;
  document?: WorkbenchDocumentV1;
}>;
```

- [ ] **Step 1: Write a failing navigation suite** for open all, singleton focus, Agent Session Open to Side, split/move/join/close, zoom, reopen, keyboard group/tab traversal, left-rail non-navigation, auxiliary object routing, roster select versus open, and all-or-nothing close guards.
- [ ] **Step 2: Write Overview tests** using real container sizes for Auto/Grid/Single, scrolling explicit Grid, persisted Single, debounce/hysteresis, selected-target independence, and focused-agent fallback. Expose `data-overview-mode="grid|single"` only as semantic state.
- [ ] **Step 3: Write recovery tests** for exact mock restore, shell sizes, unknown inert surface, missing-agent placeholder actions, backup notice, future-schema read-only behavior, Reset Surface, and guarded Reset Workbench.
- [ ] **Step 4: Add `test:e2e:workbench`** and run it with `VITE_WARDIAN_WORKBENCH=1`, then run the full browser suite in the same flagged configuration. **Expected:** PASS; no browser test makes filesystem or PTY claims.
- [ ] **Step 5: Commit** `test(workbench): cover tabs splits overview and recovery`.

### Task 17: Add native persistence, desktop/remote lease, and runtime-lifecycle proof

**Files:**
- Create: `e2e-native/lib/workbench.mjs`
- Create: `e2e-native/tests/workbench-persistence-native.test.mjs`
- Create: `e2e-native/tests/terminal-presentation-broker-native.test.mjs`
- Create: `e2e-native/tests/workbench-runtime-lifecycle-native.test.mjs`
- Modify: `e2e-native/tests/terminal-geometry-sweep-native.test.mjs`
- Modify: `e2e-native/tests/terminal-rendering-native.test.mjs`
- Modify: `e2e-native/tests/remote-gateway-native.test.mjs`
- Modify: `package.json`

- [ ] **Step 1: Replace native `selectGridView` helpers** with `openWorkbenchSurface(driver, "agents-overview")` using semantic roles/data attributes.
- [ ] **Step 2: Prove persistence with one isolated home.** Save two revisions, inspect primary/backup, restart, verify exact tree/groups/tabs/IDs, corrupt primary and recover backup, then write schema 99 and prove both files remain byte-identical.
- [ ] **Step 3: Prove desktop owner/mirror.** Open two same-agent presentations; passive mount/focus/observer do not activate; explicit begin/ack does; mirror input/resize is nonfatal; canonical geometry is stable; output/geometry sequence has no gaps through races/timeouts/disconnect.
- [ ] **Step 4: Prove desktop→authenticated remote→desktop.** Desired remote geometry does not resize while mirror; stale remote input/resize returns lease state without closing socket; activation and fallback apply the correct geometry.
- [ ] **Step 5: Prove presentation/runtime separation.** Close all Overview/Agent Session presentations, verify mock agent/PTY alive, reopen with output continuity. Run safe mode once and prove it neither flattens nor rewrites the split tree.
- [ ] **Step 6: Add `test:e2e:native:workbench`; run** native setup, a debug no-bundle build compiled with `VITE_WARDIAN_WORKBENCH=1`, and the three focused files. **Expected:** PASS with zero infrastructure skips and no real provider.
- [ ] **Step 7: Commit** `test(native): prove workbench persistence and terminal leases`.

### Task 18: Rebaseline production workbench performance with fail-closed fixtures

**Files:**
- Modify: `scripts/measure-workbench-performance.mjs`
- Create: `scripts/fixtures/workbench-performance-v1.json`
- Create: `src/config/workbenchPerformanceScript.test.ts`
- Modify: `docs/research/workbench-navigation/dockview-evaluation.md`
- Create/modify: `docs/research/workbench-navigation/workbench-performance-baseline.json`
- Delete after replacement: `scripts/measure-view-performance.mjs`
- Modify: `package.json`

- [ ] **Step 1: Test fail-closed behavior.** Unset/empty/production-default/non-absolute `WARDIAN_HOME` exits nonzero with `Refusing to benchmark without an explicit isolated WARDIAN_HOME.`
- [ ] **Step 2: Use a deterministic fixture** with 20 tabs/four groups, 20 agents, one owner/three mirrors, Agents Overview, Graph, Garden, Queue, Library, and Workflows.
- [ ] **Step 3: Emit observed values** for startup restore, tab switch, group focus, terminal output commit/gaps, Overview settle, heavy-surface resume, max React commit, renderer/WebGL peaks, and bundle gzip delta. Never hardcode observed results.
- [ ] **Step 4: Enforce initial gates:** restore p95 ≤1500 ms, tab p95 ≤100 ms, group p95 ≤75 ms, output commit p95 ≤50 ms, zero stream gaps, Overview settle p95 ≤300 ms, heavy resume p95 ≤500 ms, max commit ≤50 ms, bundle delta ≤250 KiB, xterm ≤24, WebGL ≤12.
- [ ] **Step 5: Run** `npm run perf:workbench` and `npm run perf:workbench:check` with `VITE_WARDIAN_WORKBENCH=1` and an explicit workspace-local `.tmp/workbench-performance` or OS temp home. **Expected:** baseline JSON written and gates pass. If a gate fails, optimize or document/review a justified threshold change; never silently widen it.
- [ ] **Step 6: Finalize the Promote decision** in the evaluation document and commit `perf(workbench): establish navigation cutover baseline`.

### Task 19: Cut over only after browser, native, safe-mode, and performance gates pass

**Prerequisites:** Tasks 15-18 are green in the flagged workbench. Do not start this task on partial proof.

**Files:**
- Modify: `src/views/App.tsx`
- Modify: `src/layout/titlebar/CustomTitleBar.tsx`
- Modify: `src/layout/titlebar/CustomTitleBar.test.tsx`
- Delete: `src/layout/titlebar/WorkspaceTabs.tsx`
- Delete: `src/layout/titlebar/WorkspaceTabs.test.tsx`
- Modify: `src/styles/App.css`
- Delete/modify: `src/config/workbenchFlags.ts`
- Modify: `src/config/workbenchFlags.test.ts`
- Create: `scripts/verify-workbench-cutover.mjs`
- Create: `src/config/workbenchCutoverCheck.test.ts`
- Modify: `package.json`
- Modify: `e2e/playwright.config.ts` and native build setup to remove the now-unnecessary developer flag

- [ ] **Step 1: Re-run the prerequisite gates with `VITE_WARDIAN_WORKBENCH=1` set in the current shell:** `npm run test:e2e:workbench`, targeted native build/tests, and `npm run perf:workbench:check`. **Expected:** PASS. Any failure returns to the owning task; do not cut over.
- [ ] **Step 2: Prove safe mode before deletion.** Normal → `WARDIAN_WORKBENCH_SAFE_MODE=1` → normal preserves the workbench bytes/tree exactly; safe mode can change the active group without flattening splits and uses the same persistence/CAS path.
- [ ] **Step 3: Remove the legacy navigation path.** Delete `WorkspaceTabs`, `ViewMode`, `setViewMode`, `viewMode`, `CACHED_CANVAS_VIEWS`, cached global switching, old Ctrl+Tab cycling, and the default-off `VITE_WARDIAN_WORKBENCH` branch. Workbench becomes the only model; retain runtime safe mode.
- [ ] **Step 4: Add the final cutover verifier.** It fails on the audited legacy symbols/selectors and direct desktop surface-launch button clicks across tracked `src`, `e2e`, `e2e-native`, and `scripts`; allow only documented non-desktop-navigation matches with reasons.
- [ ] **Step 5: Run** focused App/titlebar/safe-mode/cutover tests, `npm run check:workbench-cutover`, `npm run lint`, `npm run build`, full browser E2E, and targeted native workbench E2E without the developer flag. **Expected:** PASS and all 25 audit entries are migrated/removed/intentionally unrelated.
- [ ] **Step 6: Commit** `feat(workbench): make surface navigation canonical`.

## Phase 5 — Documentation, evidence, CI, and PR

### Task 20: Publish guides and capture real feature evidence

**Files:**
- Create: `docs/guide/workbench.md`
- Create: `docs/guide/agents-overview.md`
- Create: `docs/developer/workbench-surfaces.md`
- Create: `docs/developer/terminal-presentation-broker.md`
- Create from real capture: `docs/assets/screenshots/workbench-navigation/tabs-and-splits.png`
- Modify: `docs/guide/ui-overview.md`
- Modify: `docs/guide/grid.md`
- Modify: `docs/guide/dashboard.md`
- Modify: `docs/guide/watchlists.md`
- Modify: `docs/guide/queue.md`
- Modify: `docs/guide/command-panel.md`
- Modify: `docs/guide/getting-started.md`
- Modify: `docs/guide/index.md`
- Modify: `docs/developer/architecture.md`
- Modify: `docs/developer/state-management.md`
- Modify: `docs/developer/pty-lifecycle.md`
- Modify: `docs/developer/native-e2e.md`
- Modify: `docs/developer/index.md`
- Modify: `docs/.vitepress/config.ts`
- Modify: `docs/public/llms.txt`
- Modify: `README.md`
- Modify: `scripts/capture-doc-screenshots.mjs`
- Modify: `scripts/capture-readme-demo-real.mjs`

- [ ] **Step 1: Document the user model:** `+`, Home, Quick Open, commands, pane tabs/splits/move/join/zoom/reopen, auxiliary rail boundary, roster targeting versus Open/Open to Side, Overview modes, restore/placeholders/reset, explicit terminal activation, and presentation close versus agent kill.
- [ ] **Step 2: Keep `grid.md` as a compatibility entry** explaining Grid is now an Agents Overview mode, not a global page.
- [ ] **Step 3: Document developer contracts:** registry/open/render/runtime/close policies, size/version limits, persistence migrations, adapter boundary/safe mode, broker generation/epoch/snapshot/sequence/geometry, renderer budgets, and test-layer boundaries.
- [ ] **Step 4: Update capture automation** to semantic workbench helpers and remove every fixed titlebar click.
- [ ] **Step 5: Capture a real seeded 1920x1080 screenshot** to `e2e/screenshots/workbench-navigation/<timestamp>/tabs-and-splits.png`. It must show two groups, pane-local tabs, Agents Overview with agents, another surface, unchanged left rail, and right roster. Copy the curated image into the docs asset path; never use a generated mockup.
- [ ] **Step 6: Run** `npm run docs:screenshots`, `npm run docs:check-llms`, and `npm run docs:build`. **Expected:** PASS.
- [ ] **Step 7: Commit** `docs(workbench): document surface navigation and terminal activation`.

### Task 21: Add CI gates, run full verification, open the PR, and watch it green

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `src/config/ciWorkflow.test.ts`
- No product changes after final verification begins.

- [ ] **Step 1: Run browser E2E on pull requests** and add a Windows targeted `native-workbench-smoke` job using an explicit runner-temp Wardian home. Upload native artifacts on failure. Keep full native coverage local/manual if CI duration requires, but the persistence/broker/lifecycle focused suite is required.
- [ ] **Step 2: Verify CI contracts** require frontend/backend, docs, screenshot gate, browser workbench, targeted native workbench, and cutover check. Run focused config tests and commit `ci(workbench): gate navigation and terminal continuity`.
- [ ] **Step 3: Run the complete local gate:** `npm run lint`, `npm run test`, `npm run build`, `npm run docs:check-llms`, `npm run docs:build`, `npm run test:e2e`, `npm run check:workbench-cutover`, `npm run perf:workbench:check`, native setup/build, targeted workbench native, full native E2E, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace -- --test-threads=1`, and `cargo check --workspace`. **Expected:** every command exits 0; no infrastructure skips for required focused native tests.
- [ ] **Step 4: Run safety checks:** `git diff --check origin/main...HEAD`, intended `git status --short`, changed-file secrets scan, and verify no `.env`, credentials, production-home data, temp homes, or native driver artifacts are tracked.
- [ ] **Step 5: Push the branch** and verify the committed screenshot returns HTTP 200 at an immutable GitHub raw URL using the screenshot commit SHA.
- [ ] **Step 6: Build the PR body from `.github/PULL_REQUEST_TEMPLATE.md`.** Link/fix #513-#523 as actually completed, explain why the discarded Site/Cohort model was removed, list exact verification, and embed:

  ```markdown
  ![Wardian workbench with pane-local tabs, split surfaces, Agents Overview, auxiliary rail, and agent roster](https://raw.githubusercontent.com/wardian-app/Wardian/<screenshot-commit-sha>/docs/assets/screenshots/workbench-navigation/tabs-and-splits.png)
  ```

- [ ] **Step 7: Run** `PR_BODY=<body> npm run check:frontend-screenshot -- origin/main HEAD`. **Expected:** `Frontend screenshot evidence found in the PR body.`
- [ ] **Step 8: Create** PR title `feat(workbench): replace fixed navigation with restorable surface tabs`, then run `gh pr checks <number> --repo wardian-app/Wardian --watch --fail-fast`. Fix failures in scoped atomic commits and re-run until all required checks pass.
- [ ] **Step 9: Create the timed cleanup issue** `Remove workbench safe mode and terminal compatibility adapters before Wardian 0.6.0`, requiring one stable 0.5.x workbench release and recovery-report review before removing safe mode, raw desktop command adapters, and remote v1.

## Completion Definition

The epic is complete only when the workbench is Wardian's sole navigation model; every current surface is reachable without a fixed launcher; splits/tabs/zoom/restore/placeholder/reset are durable and keyboard-accessible; Agents Overview Auto/Grid/Single behaves from container geometry; same-session desktop/remote presentations have one explicit owner and stable mirrors; closing presentations never kills agents; the full repository/native/performance suites pass; docs and a real embedded screenshot ship; #513-#523 accurately close or explicitly defer remaining work; and the PR is green.
