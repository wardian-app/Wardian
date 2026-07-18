# Files Conventional Editor Core Implementation Plan

> **For Codex:** REQUIRED SUB-SKILL: Use `subagent-driven-development` to execute this plan task by task with an independent review after every task.

**Goal:** Replace the Files surface's Preview / Changes / Draft modes with a conventional shared editor, explicit guarded saves, durable recovery, and an inline/full diff lens that ordinary files and later artifact workflows can share.

**Architecture:** A canonical `resource_id` owns one frontend editor session shared by every pane showing that resource. The native Files runtime remains authoritative for authorization, revisions, guarded writes, and recovery. Presentation (rendered/editor), comparison visibility, and pane layout remain surface state; buffer text, dirty state, save state, and recovery ownership remain resource state. Monaco uses stable resource models so switching presentation or panes never destroys an unsaved buffer.

**Tech stack:** React 19, TypeScript, Zustand/external stores, Monaco, Vitest, Playwright, Rust/Tauri, `diffy`, existing Files resource runtime.

**Approved spec:** `docs/specs/2026-07-17-files-editor-review-interaction.md`

**Issue:** #392 (related: #393, #395, #513)

## Scope and invariants

- Remove the visible Preview / Changes / Draft mode tabs. Do not preserve them under different names.
- One canonical authorized file owns one shared working buffer. Duplicate panes observe the same buffer.
- Only closing the final presentation of a dirty resource prompts. Closing several panes/groups is evaluated as one transaction.
- Saving is explicit and guarded by the exact authorized target, base revision token, and base content hash.
- Durable recovery is backend-owned and does not authorize a path by itself.
- Rendered/editor is a presentation toggle. Changes are inline annotations and an optional comparison lens.
- Diff layout uses deterministic thresholds: side-by-side at 720 px or wider, unified from 560–719 px, summary below 560 px.
- The Files overflow control matches the Workbench titlebar ellipsis: Lucide `Ellipsis`, 17 px glyph, 1.75 px stroke, 26×26 px hit area.
- Do not modify the existing user-owned `package-lock.json` diff.
- Artifact persistence, the `wardian artifact` CLI, prompt checkpoint indexing, Send/Approve actions, and live HTML/SVG isolation are downstream work and are not silently folded into this plan.

## Task 1: Introduce Files state V2 and transactional close context

**Files:**
- Modify: `src/types/files.ts`
- Modify: `src/features/workbench/coreSurfaceRegistry.ts`
- Modify: `src/features/workbench/surfaceRegistry.ts`
- Modify: `src/features/workbench/useWorkbenchPersistence.ts`
- Modify: `src/features/files/fileResourceKey.ts`
- Modify: `src/features/files/filesPresentationStore.ts`
- Modify: `src/features/workbench/surfaces/dirtySurfaceGuards.ts`
- Modify: associated `*.test.ts` files

### Step 1: Write failing state and close-context tests

Cover:

- V1 migration intent: `preview` requests the renderer default; `changes` and `draft` request editor; `changes` opens comparison only when its checkpoint/artifact baseline resolves.
- V2 round-trip persistence.
- Renderer normalization follows the approved fallback order: unsupported editor → renderer default; unsupported rendered → editor when available; unavailable baseline closes comparison; an unfit side-by-side layout degrades without losing the saved preference.
- A group close receives the complete set of `closing_surface_ids`, so duplicate dirty file views only prompt when all remaining presentations close.
- Single-pane close does not prompt when another pane still presents the resource.
- Legacy state must never resurrect a Draft buffer. The shipped foundation did not persist editable Draft bytes.

### Step 2: Define V2 state

Use a discriminated, serializable contract equivalent to:

```ts
export type FilesComparisonBaseline =
  | { kind: "saved_file" }
  | { kind: "prompt_checkpoint"; checkpoint_id: string }
  | { kind: "presented_version"; version_id: string }
  | { kind: "previous_presented_version"; version_id: string };

export interface FilesSurfaceStateV2 {
  resource_kind: "file" | "artifact";
  transient_preview: boolean;
  presentation: "rendered" | "editor";
  comparison_open: boolean;
  comparison_layout_preference: "auto" | "side_by_side" | "unified";
  comparison_baseline: FilesComparisonBaseline | null;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
}
```

Keep artifact fields inert until the artifact tasks implement them. Validate all persisted strings and enum values rather than casting arbitrary JSON.

The synchronous registry restores only validated schema state. Add a Workbench persistence migration/normalization phase that can finish after the Files descriptor and baseline providers are available, then durably save normalized V2 state before recording migration completion. Current shipped V1 state contains no Draft bytes or base hash; prove that invariant and migrate legacy `draft` as a clean editor. If any legacy byte-bearing format is discovered, stop and route it through Task 3 recovery before removing it.

### Step 3: Make close guards transaction-aware

Add a `SurfaceCloseContext` to the registry guard contract containing the current surface snapshot and the complete `closing_surface_ids`. Update every invocation, including pane/group/workspace close. Existing non-Files guards may ignore the context.

The Files guard must query resource-owned dirty state through an injected interface; do not import a React component or reach into Monaco.

### Step 4: Verify and commit

Run focused Vitest files, `npm run lint`, and `npm run build`.

Commit: `feat(files): add editor surface state contract`

## Task 2: Add retained-handle guarded atomic text saves

**Files:**
- Modify: `crates/wardian-core/src/files/authorized_roots.rs`
- Modify: `crates/wardian-core/src/files/mod.rs`
- Modify: `src-tauri/src/commands/files.rs`
- Modify: `src-tauri/src/state/file_resources.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: Rust unit/integration tests beside those modules

### Step 1: Write RED authorization and race tests

Prove:

- stale revision and stale base hash cannot overwrite newer bytes;
- revoked roots and changed file identity remain unauthorized;
- symlink/junction retargeting cannot redirect a save;
- atomic replacement returns a newly authorized retained handle and opaque revision token;
- no partial bytes become visible on failure;
- unchanged content is an explicit no-op;
- watcher echo does not emit a second logical revision.
- Save As consumes a one-shot exact native save-target grant, cannot create a sibling with another name, and never retargets the source resource or an artifact identity.

### Step 2: Add the core write primitive

Add an API equivalent to:

```rust
pub fn guarded_atomic_replace_text(
    &self,
    expected_revision: &FileRevisionToken,
    expected_hash: &str,
    text: &str,
    limits: &FileResourceLimits,
) -> Result<GuardedFileWrite, FileResourceErrorV1>;
```

The implementation must lock and verify the retained handle, validate the expected token/hash, stage a sibling temporary file, preserve applicable permissions, flush, reverify the original binding immediately before replacement, atomically replace the canonical target, and call the existing safe same-target reauthorization path. Never reopen the path before the authorization checks or weaken the retained-handle contract.

### Step 3: Add the Tauri save command

Request fields:

```text
resource_id, subscription_id, expected_revision, buffer_base_hash, text
```

Return a tagged result with `saved`, `unchanged`, or `stale_conflict`. The saved/unchanged variants return the current opaque revision and content hash. Stale conflict returns current metadata only; it must not disclose bytes through an unauthorized or expired subscription.

Serialize save, explicit refresh, watcher refresh, and final-close cleanup through one per-resource operation mutex. A successful save emits exactly one resource revision event.

### Step 4: Add exact-target Save As

Add a native save dialog command that mints a one-shot grant bound to the verified canonical parent identity and exact selected basename. The grant authorizes creating or replacing only that selected ordinary-file target, expires promptly, and is consumed on use. It must not grant the parent directory and must fail closed if the parent identity or selected target binding changes.

`save_file_resource_as_text` atomically writes the submitted buffer through that grant and returns a new ordinary file grant/resource identity. It never retargets the current file resource or artifact thread. Opening the returned resource and closing the source remains a frontend transaction so a failed open cannot lose the original session.

### Step 5: Verify and commit

Run focused core/Tauri tests with `CARGO_BUILD_JOBS=1`, then `cargo check --workspace` and strict Clippy.

Commit: `feat(files): add guarded atomic text saves`

## Task 3: Add durable recovery and three-way stale-file resolution

**Files:**
- Modify: workspace `Cargo.toml` / `Cargo.lock` as required for `diffy`
- Add or modify: `src-tauri/src/commands/file_recovery.rs`
- Modify: `src-tauri/src/commands/files.rs`
- Modify: `src-tauri/src/state/file_resources.rs`
- Modify: `src-tauri/src/lib.rs`
- Add: focused Rust tests

### Step 1: Write RED recovery tests

Cover checkpoint create/update, app restart, authorized restore, read-only restore while file authorization is unavailable, discard, recovery compare-and-swap conflicts, stale file detection, clean three-way merge, overlapping-conflict markers, and cleanup after successful save. Prove recovery access cannot read current file bytes, write the file, or revive an expired/revoked file capability.

### Step 2: Implement the recovery store

Store backend-owned recovery records below:

```text
<WARDIAN_HOME>/files/recovery/<recovery-id>/manifest.json
<WARDIAN_HOME>/files/recovery/<recovery-id>/base.txt
<WARDIAN_HOME>/files/recovery/<recovery-id>/buffer.txt
```

The manifest records a schema version, opaque recovery id, canonical resource identity, display metadata, base content hash, base opaque revision, recovery CAS revision, originating webview scope, and timestamps. It must not grant filesystem authority.

Recovery lookup/read is separately scoped to the same Wardian app webview and exact stable `resource_key`, permitting the approved read-only recovery screen after restart even when file authorization is unavailable. That access may reveal only the saved recovery base/buffer. Saving, merging with the disk head, or reading current file bytes still requires a newly verified live file subscription for the same target. **Restore access** performs normal authorization and never revives an old file capability.

Writes use sibling temporary files plus atomic rename. Enforce text and storage limits from the Files resource limits; reject invalid UTF-8 and oversized buffers consistently.

### Step 3: Expose narrow commands

Add commands equivalent to:

- `checkpoint_file_recovery`
- `get_file_recovery`
- `discard_file_recovery`
- `merge_file_recovery`

Checkpoint accepts the expected recovery CAS revision. Merge uses `diffy` for a three-way merge of base, buffer, and current authorized bytes. Return structured clean/conflicted outcomes; never silently choose one side.

### Step 4: Verify and commit

Run focused tests, workspace check, strict Clippy, and workspace tests with serialized build jobs.

Commit: `feat(files): persist editor recovery buffers`

## Task 4: Build the shared resource-owned editor controller

**Files:**
- Add: `src/features/files/fileEditorController.ts`
- Add: `src/features/files/fileEditorController.test.ts`
- Modify: `src/features/files/filesPresentationStore.ts`
- Modify: `src/features/workbench/surfaces/dirtySurfaceGuards.ts`
- Modify: `src/views/App.tsx` or the narrow Files runtime wiring module
- Modify: relevant IPC adapters/types

### Step 1: Write RED controller tests

Prove:

- duplicate surfaces share one buffer and dirty state;
- presentation switches and component unmounts do not destroy the buffer;
- first mutation pins a transient preview;
- explicit save sends the exact base revision/hash and advances the baseline only on success;
- stale save keeps the buffer dirty and opens comparison state;
- recovery checkpoints are debounced without losing the final mutation;
- only the final close offers Save / Don't Save / Cancel;
- failed Save cancels closing;
- Don't Save discards both in-memory and durable recovery state;
- final subscription cleanup waits for close resolution.

### Step 2: Implement a resource session registry

Use an imperative resource-keyed controller with `useSyncExternalStore` (or an equivalently tear-free subscription API). Do not put editor text in the surface layout store and do not recreate a Monaco model on every resource revision.

Each session owns at least:

```text
resource_id, subscription_id, authorized revision, base hash, saved text,
working text, dirty flag, save state, stale state, recovery state, view refcount
```

The runtime may drop a clean zero-view session. A dirty zero-view session remains only long enough to resolve the close transaction/recovery checkpoint, then releases native subscriptions deterministically.

### Step 3: Wire dirty badges and close guards

Expose resource dirty state to the Workbench tab label and Files breadcrumb. Keep the close guard UI-independent by injecting save/discard methods from the controller. Ensure group close calls a single decision per dirty resource even when several panes share it.

### Step 4: Verify and commit

Run focused controller/registry tests, all frontend tests, lint, and build.

Commit: `feat(files): share editor sessions across panes`

## Task 5: Replace mode tabs with the conventional Monaco editor header

**Files:**
- Rename/replace: `src/features/files/FilesModeBar.tsx` → `FilesHeader.tsx`
- Rename/replace: `src/features/files/FilePreview.tsx` → `FileContentHost.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/renderers/MonacoTextRenderer.tsx`
- Modify: `src/features/files/rendererRegistry.ts`
- Modify: `src/features/files/FilesSurface.css`
- Modify: component tests and `e2e/tests/files-surface-foundation.spec.ts`

### Step 1: Write RED interaction tests

Cover:

- Markdown/HTML/active SVG default rendered and show the Book/Pencil current-state control;
- source-only text opens editable Monaco with no redundant toggle;
- images/PDF stay read-only with no misleading Pencil;
- the icon reflects current state, while label/tooltip describe the action;
- keyboard Save invokes the controller (`Ctrl+S`/`Cmd+S`);
- the overflow menu exposes Save and Save As only when supported;
- Save As opens the newly created ordinary resource only after the native operation succeeds and leaves artifact identity unchanged;
- dirty dots appear in tab and breadcrumb;
- the compact overflow geometry matches the Workbench titlebar control;
- narrow panes retain accessible controls without overlapping.

### Step 2: Implement the compact header

Remove all Preview / Changes / Draft labels and styling. Keep path display cross-platform and Explorer-consistent; never show Windows verbatim prefixes such as `\\?\`.

Use Lucide `BookOpen`/`Pencil` for current presentation and `Ellipsis` at exactly 17 px, 1.75 stroke, 26×26 hit area. Use theme variables only. Do not add a permanent text Save button.

### Step 3: Make Monaco a stable editable model

Key the Monaco model by canonical resource identity, not by revision. Bind it to the shared controller, preserve undo/redo and selection across presentation switches, and suppress feedback loops when applying external/controller state. Set language from the renderer registry. Route keybindings through the active resource session.

Rendered presentation always renders the working buffer when dirty so Book/Pencil switching is lossless and unsurprising.

### Step 4: Verify and commit

Run focused component tests, all frontend tests, lint, build, and the Files browser E2E.

Commit: `feat(files): add conventional editable file surface`

## Task 6: Add inline saved-file changes and the full comparison lens

**Files:**
- Add: `src/features/files/fileDiffModel.ts`
- Add: `src/features/files/fileDiffModel.test.ts`
- Add: `src/features/files/FileComparisonLens.tsx`
- Add: `src/features/files/FileComparisonLens.test.tsx`
- Modify: `src/features/files/renderers/MonacoTextRenderer.tsx`
- Modify: `src/features/files/FilesHeader.tsx`
- Modify: `src/features/files/FilesSurface.css`

### Step 1: Write RED diff and layout tests

Prove:

- clean buffers have no change annotations;
- inserts, modifications, and deletions map to stable line decorations against the saved-file baseline;
- editing updates annotations without replacing the editor model;
- comparison open/close is independent of rendered/editor presentation;
- auto layout resolves side-by-side at ≥720 px, unified at 560–719 px, and compact summary below 560 px;
- explicit layout preferences degrade safely when the pane cannot fit them;
- stale-file resolution can accept current disk, keep working buffer, or apply a clean merge without writing until Save.

### Step 2: Add editor annotations

Decorate changed lines and the gutter in the ordinary editor. Keep the decoration palette semantic and theme-driven. Add one compact changes icon/badge to the header when changes exist; its accessible label opens the comparison lens.

### Step 3: Add the comparison lens

Use Monaco's diff editor for full comparison. The left/original model is the selected baseline, and the right/modified model is the live shared buffer. Models must be resource/baseline keyed and disposed only when no consumer remains.

For this core task, implement the `saved_file` baseline fully. Preserve the other baseline variants in state and render an explicit unavailable state until their downstream providers are registered; never fake them using saved-file bytes.

### Step 4: Verify and commit

Run focused diff/component tests, all frontend tests, lint, build, and Files browser E2E.

Commit: `feat(files): add inline changes and comparison lens`

## Task 7: Prove native behavior, document the shipped interaction, and align downstream plans

**Files:**
- Modify: `e2e-native/tests/files-resource-native.test.mjs`
- Modify: `e2e/tests/files-surface-foundation.spec.ts`
- Add/update: feature-specific screenshot tests and output under `e2e/screenshots/files-editor/<timestamp>/`
- Modify: `docs/guide/` or `docs/developer/` Files documentation
- Modify: `docs/superpowers/plans/2026-07-16-artifact-lifecycle.md`
- Modify: `docs/superpowers/plans/2026-07-16-artifact-review-and-prompt-changes.md`
- Modify: `docs/superpowers/plans/2026-07-16-live-artifact-isolation-and-files-activation.md`

### Step 1: Add native acceptance proof

Through real Tauri IPC and filesystem operations, prove authorized edit → save → revision update, stale conflict after external edit, exact-target/revocation rejection, recovery across app/runtime recreation, clean/conflicted merge, and subscription cleanup. Browser mocks are not sufficient for these claims.

### Step 2: Add browser UX proof

Prove rendered/editor toggle semantics, editable Monaco, shared buffer across panes, dirty badges, final-close prompt, inline annotations, comparison layout thresholds, and compact header behavior. Capture at least one representative feature screenshot suitable for embedding in PR #673.

### Step 3: Update documentation and downstream plans

Document explicit save, recovery, current-state Book/Pencil semantics, inline changes, and the comparison lens using cross-platform examples.

Amend the three downstream artifact plans so they consume this editor core and do not reintroduce Preview / Changes / Draft:

1. artifact lifecycle owns persistence, CLI presentation, provenance, and attention;
2. artifact review owns prompt checkpoint adapters, comments, Send to agent, and approval;
3. live isolation owns HTML/SVG sandboxing and final activation.

### Step 4: Run complete required verification

Run, without overlapping Cargo invocations:

```powershell
npm run lint
npm run test
npm run build
npm run test:e2e -- e2e/tests/files-surface-foundation.spec.ts
$env:CARGO_BUILD_JOBS='1'; cargo check --workspace
$env:CARGO_BUILD_JOBS='1'; cargo clippy --workspace --all-targets -- -D warnings
$env:CARGO_BUILD_JOBS='1'; cargo test --workspace
npm run test:e2e:native:fast -- e2e-native/tests/files-resource-native.test.mjs
```

Also run scoped format, credential, diff, docs-link, and screenshot-policy checks used by CI. Confirm `git status` contains only intended work plus the preserved user-owned `package-lock.json` change.

### Step 5: Independent whole-branch review and commit

Review the exact range from `98f527bf` through HEAD for spec compliance, authorization integrity, race safety, UX regressions, and test gaps. Resolve every Critical or Important finding and rerun affected gates.

Commit: `test(files): prove editor and review workflow`

## Completion criteria

- Files has no Preview / Changes / Draft mode UI or internal workflow dependency.
- Text resources edit conventionally in Monaco and save explicitly.
- Duplicate panes share a single working buffer and final-close semantics.
- Saves cannot escape authorization or overwrite a stale revision unnoticed.
- Dirty buffers survive crashes through bounded backend recovery.
- Ordinary editor changes are visible inline and in a responsive full comparison lens.
- Existing renderers, resource subscriptions, path display, pane behavior, and tab/titlebar behavior remain regression-covered.
- Downstream artifact plans build on this core without duplicating or contradicting it.
