# Artifact Review and Prompt Changes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add trustworthy Changes since prompt, durable user drafts, conflict-safe apply, line/spatial review, approval, and structured agent feedback to the artifact-enabled Files surface.

**Architecture:** A shared backend `PromptChangeTracker` maintains content-addressed indexes for each agent's authorized roots and captures durable checkpoints immediately before user-originated delivery. A backend draft/review service stores proposed text, computes three-way merges, applies atomically, and sends concise review references. React adds Changes and Draft modes plus a responsive review drawer; it never performs authoritative merge, apply, anchoring, or comment-state transitions.

**Tech Stack:** Rust, notify, SHA-256, diffy, serde, Tauri delivery services, Monaco Diff Editor, React 19, Zustand, Vitest, Playwright, and native E2E.

## Global Constraints

- Plans 1 and 2 must be merged first. Reuse their authorization, stable revisions, blob store, artifact origin, and Files resource controller.
- Label temporal diffs **Changes since prompt**. Never label them “Agent changes” or claim authorship.
- Capture a checkpoint immediately before Wardian delivers a user-originated prompt. Inter-agent control messages, terminal escape replies, and provider output do not advance it.
- Prompt delivery must not synchronously crawl a workspace. If the incremental index is not ready, deliver with a durable `baseline_unavailable` association.
- Index `AgentConfig.folder` and `include_directories`; exclude `system_include_directories`, Wardian state, VCS internals, and default high-churn caches.
- Draft text is backend-owned and durable. Entering Draft pins a transient tab.
- Apply and Send are independent operations. Send never writes the file; Apply never sends a message.
- Apply repeats the base/current hash check and atomically replaces the target. Never silently overwrite drift.
- Non-overlapping drift rebases automatically. Overlapping drift produces an explicit three-way conflict result rendered by Monaco.
- Comment state transitions are explicit: `open`, `agent_marked_addressed`, `resolved`, or `outdated`. Content changes never imply semantic resolution.
- Text anchors are line/column ranges; image/PDF anchors use intrinsic normalized coordinates and a trusted overlay.
- The Files launcher card remains reserved through this plan.

---

### Task 1: Content-addressed indexes and durable prompt checkpoints

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/wardian-core/Cargo.toml`
- Modify: `crates/wardian-core/src/lib.rs`
- Create: `crates/wardian-core/src/changes/mod.rs`
- Create: `crates/wardian-core/src/changes/content_index.rs`
- Create: `crates/wardian-core/src/changes/checkpoint.rs`
- Create: `crates/wardian-core/src/changes/diff.rs`
- Modify: `crates/wardian-core/src/artifacts/models.rs`
- Modify: `crates/wardian-core/src/artifacts/store.rs`
- Test: `crates/wardian-core/src/changes/content_index.rs`
- Test: `crates/wardian-core/src/changes/checkpoint.rs`
- Test: `crates/wardian-core/src/changes/diff.rs`

**Interfaces:**
- Consumes: authorized canonical roots, shared immutable blob storage, filesystem metadata/events, and the artifact checkpoint directory.
- Produces: `ContentIndex`, `ContentIndexEntryV1`, `PromptCheckpointV1`, `PromptBaselineState`, `PathChangeV1`, and `compare_checkpoint_to_index`.

- [ ] **Step 1: Add failing index/checkpoint/diff tests**

Cover added, modified, deleted, and renamed files; identical content deduplication; ignore rules; explicitly tracked artifact override; folder/additional-root scope; system-include exclusion; stable flush; restart load; baseline unavailable; and paths with the same prefix but different components.

```rust
#[test]
fn rename_is_detected_by_content_identity() {
    let before = index_with([entry("old/report.md", "hash-a")]);
    let after = index_with([entry("new/report.md", "hash-a")]);
    assert_eq!(compare_indexes(&before, &after), vec![PathChangeV1::Renamed {
        old_path: "old/report.md".into(),
        new_path: "new/report.md".into(),
        content_hash: "hash-a".into(),
    }]);
}
```

- [ ] **Step 2: Run focused core tests and confirm the changes module is missing**

Run: `cargo test -p wardian-core changes:: -- --test-threads=1`

Expected: compilation fails because `wardian_core::changes` does not exist.

- [ ] **Step 3: Define compact content-addressed schemas**

Each index entry contains canonical root ID, normalized relative path, content hash, byte size, and modified time. A checkpoint stores:

```rust
pub struct PromptCheckpointV1 {
    pub schema: u8,
    pub checkpoint_id: String,
    pub session_id: String,
    pub input_id: String,
    pub origin_kind: UserInputOriginKind,
    pub created_at_ms: u64,
    pub index_root_hash: Option<String>,
    pub baseline_state: PromptBaselineState,
}
```

`PromptBaselineState` serializes `ready` or `{ "unavailable": { "reason": "index_not_ready" } }`. Store checkpoint JSON at `artifacts/checkpoints/<checkpoint-id>.json`; store deduplicated index nodes and file bytes in the existing blob store.

- [ ] **Step 4: Implement incremental comparison and ignores**

Default exclusions include `.git`, `.hg`, `.svn`, `.wardian`, `node_modules`, `target`, `dist`, `build`, `.next`, coverage caches, and editor caches. Match complete path segments. An explicitly tracked artifact path is indexed despite a default exclusion, but only while it remains within an authorized root.

- [ ] **Step 5: Keep prompt capture O(pending events)**

`ContentIndex::flush_stable_pending()` hashes only stable pending paths, commits a new root, and returns its hash. Initial scanning is a separate state transition and never occurs inside checkpoint capture. Persist root nodes before checkpoint JSON.

- [ ] **Step 6: Run focused and crate tests**

Run: `cargo test -p wardian-core changes:: -- --test-threads=1`

Run: `cargo test -p wardian-core -- --test-threads=1`

Expected: index, ignore, override, durable checkpoint, rename, and unavailable-baseline tests pass.

- [ ] **Step 7: Commit the shared change model**

```bash
git add Cargo.toml Cargo.lock crates/wardian-core
git commit -m "feat(changes): add prompt checkpoint index"
```

### Task 2: Backend PromptChangeTracker and pre-delivery ordering

**Files:**
- Create: `src-tauri/src/state/prompt_changes.rs`
- Modify: `src-tauri/src/state/mod.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Create: `src-tauri/src/delivery/user_prompt.rs`
- Modify: `src-tauri/src/delivery/mod.rs`
- Modify: `src-tauri/src/commands/terminal.rs`
- Modify: `src-tauri/src/commands/agent.rs`
- Test: `src-tauri/src/state/prompt_changes.rs`
- Test: `src-tauri/src/delivery/user_prompt.rs`
- Test: `src-tauri/src/commands/terminal.rs`

**Interfaces:**
- Consumes: agent lifecycle/config updates, authorized roots, filesystem watch events, `submit_live_surface_prompt`, and conversation input IDs.
- Produces: `PromptChangeTracker`, `submit_user_prompt_with_checkpoint`, `UserPromptDeliveryResultV1`, and optional `checkpoint_id`/baseline state on prompt delivery details.

- [ ] **Step 1: Add failing ordering and scope tests**

Use an injectable delivery recorder to assert `flush -> persist checkpoint association -> provider submit`. Cover index-not-ready delivery, delivery failure retaining the checkpoint, two simultaneous prompts serialized per session, config root changes, agent removal cleanup, and raw/inter-agent inputs not creating checkpoints.

```rust
assert_eq!(recorder.events(), [
    "index_flushed",
    "checkpoint_persisted",
    "provider_submit_started",
]);
```

- [ ] **Step 2: Run focused Tauri tests and confirm no tracker exists**

Run: `cargo test -p Wardian prompt_changes -- --test-threads=1`

Expected: compilation fails because the tracker and wrapper are missing.

- [ ] **Step 3: Implement one tracker per AppState**

`PromptChangeTracker` owns per-session index state, root watchers, stable pending paths, tracked artifact overrides, and readiness. Start/rebind it when an agent becomes active or its config changes; stop/release it on agent removal. Reuse Plan 1 watcher normalization and exclusions instead of opening component-owned watchers.

- [ ] **Step 4: Centralize user-originated delivery**

Add:

```rust
pub async fn submit_user_prompt_with_checkpoint(
    app: Option<&AppHandle>,
    state: &AppState,
    request: UserPromptDeliveryRequest,
) -> Result<UserPromptDeliveryResultV1, UserPromptDeliveryError>;
```

Generate `input_id`, acquire the existing per-session delivery lock, flush stable index events, persist the checkpoint/input association, then call `submit_live_surface_prompt` without reacquiring the same lock. Refactor lock ownership if necessary so the ordering cannot deadlock.

- [ ] **Step 5: Route only user-originated paths through the wrapper**

`submit_prompt_to_agent` uses the wrapper with `origin_kind: direct_prompt`. Plan 3 review delivery later uses `artifact_review`. Do not wrap `inject_session_input`, provider protocol replies, broadcast raw input, mailbox drain, or inter-agent `ControlRequest::SendMessage`.

- [ ] **Step 6: Return checkpoint metadata without breaking old callers**

Add optional snake_case fields to the delivery DTO: `checkpoint_id` and `prompt_baseline_state`. Existing clients can ignore them. Archive the input/checkpoint association before provider submit and record delivery outcome afterward.

- [ ] **Step 7: Run focused tests**

Run: `cargo test -p Wardian prompt_changes -- --test-threads=1`

Run: `cargo test -p Wardian commands::terminal -- --test-threads=1`

Expected: ordering, lock, ready/unavailable, config-rebind, and no-false-checkpoint tests pass.

- [ ] **Step 8: Commit checkpointed delivery**

```bash
git add src-tauri/src/state src-tauri/src/delivery src-tauri/src/commands/terminal.rs src-tauri/src/commands/agent.rs
git commit -m "feat(delivery): checkpoint user prompts before submit"
```

### Task 3: Changes since prompt API and Files Changes mode

**Files:**
- Create: `src-tauri/src/commands/changes.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/types/files.ts`
- Modify: `src/features/files/fileResourceClient.ts`
- Create: `src/features/files/usePromptChanges.ts`
- Create: `src/features/files/usePromptChanges.test.tsx`
- Create: `src/features/files/renderers/MonacoDiffRenderer.tsx`
- Create: `src/features/files/renderers/MonacoDiffRenderer.test.tsx`
- Create: `src/features/files/renderers/BinaryVersionCompare.tsx`
- Create: `src/features/files/renderers/BinaryVersionCompare.test.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesModeBar.tsx`
- Modify: `src/features/files/rendererRegistry.ts`
- Modify: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: `PromptChangeTracker`, selected agent/checkpoint context, current file revision, artifact selected version, Monaco diff limits, and resource tickets.
- Produces: `get_file_prompt_changes`, `FilePromptChangesV1`, Changes mode, baseline-unavailable UI, and image/PDF previous/current comparison.

- [ ] **Step 1: Add failing API and renderer tests**

Cover modified/added/deleted/renamed text paths, current agent context, explicitly selected checkpoint, no context, unavailable baseline, stale checkpoint, line limits, image/PDF metadata, and exact **Changes since prompt** labeling.

```ts
expect(screen.getByRole("tab", { name: "Changes since prompt" })).toBeEnabled();
expect(screen.queryByText(/agent changes/i)).not.toBeInTheDocument();
```

- [ ] **Step 2: Run focused tests and confirm Changes remains disabled**

Run: `npm run test -- --run src/features/files/usePromptChanges.test.tsx src/features/files/renderers/MonacoDiffRenderer.test.tsx src/features/files/renderers/BinaryVersionCompare.test.tsx src/features/files/FilesSurface.test.tsx`

Expected: missing modules and disabled-mode assertions fail.

- [ ] **Step 3: Add a typed comparison command**

`get_file_prompt_changes` accepts resource key, canonical path, `agent_id`, optional `checkpoint_id`, and current revision. It returns baseline state, path change kind, old/new descriptors, revision-bound read sources, checkpoint metadata, and a precise unavailable reason. It never substitutes first-observed or presented-version comparisons under a prompt-scoped label.

- [ ] **Step 4: Implement Monaco line comparison**

Load old/current models lazily, key them by content hash, enforce 5 MiB/100,000-line limits per side, and dispose by reference count. Added/deleted files use an empty counterpart. A rename displays both paths. Keep the diff editor read-only; entering Draft occurs through the mode bar.

- [ ] **Step 5: Implement binary version comparison**

For image/PDF artifacts, Changes shows selected presented version and current working state side by side with dimensions/page count, byte size, hashes, and timestamps. Do not label it a semantic visual diff. Ordinary binary files without a checkpoint-readable old blob show a typed unavailable state.

- [ ] **Step 6: Enable mode only for honest comparisons**

Files mode state may persist `changes`, but restoration falls back to the unavailable view rather than Preview when the baseline is absent. The mode bar reports why. HTML/SVG source comparison is registered only after Plan 4 activates those renderers.

- [ ] **Step 7: Run focused tests**

Run: `npm run test -- --run src/features/files/usePromptChanges.test.tsx src/features/files/renderers/MonacoDiffRenderer.test.tsx src/features/files/renderers/BinaryVersionCompare.test.tsx src/features/files/FilesSurface.test.tsx`

Run: `cargo test -p Wardian changes -- --test-threads=1`

Expected: scope, labels, unavailable states, limits, and comparison tests pass.

- [ ] **Step 8: Commit Changes mode**

```bash
git add src-tauri/src/commands src-tauri/src/lib.rs src/types/files.ts src/features/files
git commit -m "feat(files): show changes since prompt"
```

### Task 4: Durable drafts, three-way merge, and atomic apply

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/wardian-core/Cargo.toml`
- Create: `crates/wardian-core/src/artifacts/draft.rs`
- Create: `crates/wardian-core/src/artifacts/merge.rs`
- Modify: `crates/wardian-core/src/artifacts/mod.rs`
- Modify: `crates/wardian-core/src/artifacts/models.rs`
- Modify: `crates/wardian-core/src/artifacts/store.rs`
- Create: `src-tauri/src/commands/drafts.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `crates/wardian-core/src/artifacts/draft.rs`
- Test: `crates/wardian-core/src/artifacts/merge.rs`
- Test: `src-tauri/src/commands/drafts.rs`

**Interfaces:**
- Consumes: artifact blob store, file revision/hash, exact file capability, shared atomic-write helpers, and prompt/version references.
- Produces: `DraftRecordV1`, `DraftMergeResultV1`, `create_or_load_draft`, `save_draft`, `rebase_draft`, `apply_draft`, `discard_draft`, and `get_draft`.

- [ ] **Step 1: Add failing persistence, merge, and apply tests**

Cover restart survival, one draft per resource/user context, non-overlapping rebase, overlapping conflict hunks, current hash drift between preview and apply, same-directory temp write/flush/replace, permission failure retaining draft, exact-picker apply, unauthorized apply, and apply without send.

```rust
#[test]
fn overlapping_changes_return_conflict_without_mutating_disk() {
    let result = merge_three_way("a\nb\n", "a\ncurrent\n", "a\nproposed\n");
    assert!(matches!(result, DraftMergeResultV1::Conflict { .. }));
    assert_eq!(std::fs::read_to_string(test_file()).unwrap(), "a\ncurrent\n");
}
```

- [ ] **Step 2: Add the merge dependency and run focused tests**

Run: `cargo add diffy -p wardian-core`

Run: `cargo test -p wardian-core artifacts:: -- --test-threads=1`

Expected: tests fail because draft/store APIs are missing.

- [ ] **Step 3: Persist backend-owned drafts**

Add `artifacts/drafts/<draft-id>.json` to the store. A record includes resource key, canonical path, base content hash/blob, current draft blob, artifact/version/checkpoint references, created/updated timestamps, and optional applied hash. Blob text is immutable/deduplicated; the record is atomic.

- [ ] **Step 4: Implement authoritative merge outcomes**

Wrap `diffy` behind `DraftMergeService` so UI never depends on library-specific conflict markers. Return `Unchanged`, `Rebased { text, new_base_hash }`, or `Conflict { base_text_ref, current_text_ref, proposed_text_ref, conflicts }`. Each conflict contains base/current/proposed line ranges.

- [ ] **Step 5: Implement atomic apply**

Re-authorize the canonical target, read current hash, merge/reject drift, create a temporary file in the target's directory, preserve appropriate permissions, write and flush bytes, atomically replace, and record `applied_hash`. A failure leaves the original file and durable draft intact.

- [ ] **Step 6: Expose typed draft commands**

Every command takes `resource_key` plus draft ID where applicable. `save_draft` uses an expected draft revision to reject stale UI writes. `apply_draft` returns applied/rebased/conflict without throwing conflict as an application error.

- [ ] **Step 7: Run core and Tauri tests**

Run: `cargo test -p wardian-core artifacts:: -- --test-threads=1`

Run: `cargo test -p Wardian drafts -- --test-threads=1`

Expected: persistence, restart, merge, authorization, atomicity, and failure-retention tests pass.

- [ ] **Step 8: Commit draft services**

```bash
git add Cargo.toml Cargo.lock crates/wardian-core src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(artifacts): add durable conflict-safe drafts"
```

### Task 5: Draft mode, close guard, and Monaco conflict UI

**Files:**
- Create: `src/features/files/useFileDraft.ts`
- Create: `src/features/files/useFileDraft.test.tsx`
- Create: `src/features/files/renderers/MonacoDraftRenderer.tsx`
- Create: `src/features/files/renderers/MonacoDraftRenderer.test.tsx`
- Create: `src/features/files/renderers/MonacoMergeRenderer.tsx`
- Create: `src/features/files/renderers/MonacoMergeRenderer.test.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesModeBar.tsx`
- Modify: `src/features/files/filesPresentationStore.ts`
- Modify: `src/features/workbench/coreSurfaceRegistry.ts`
- Modify: `src/features/workbench/coreSurfaceRegistry.test.ts`
- Modify: `src/features/workbench/navigationService.ts`
- Test: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: Task 4 draft commands, Monaco, transient pinning, Files dirty presentation store, and Workbench dirty prompt.
- Produces: Draft mode, debounced durable save, Apply to file, conflict resolution, and Keep Draft/Discard Draft/Cancel close behavior.

- [ ] **Step 1: Add failing Draft and close tests**

Assert entering Draft pins transient previews, loads/creates a durable draft, marks the tab dirty only for unapplied/unsent changes, survives remount, applies without sending, renders a three-way conflict, keeps the draft on close, discards only after confirmation, and Cancel leaves the surface open.

- [ ] **Step 2: Run focused frontend tests**

Run: `npm run test -- --run src/features/files/useFileDraft.test.tsx src/features/files/renderers/MonacoDraftRenderer.test.tsx src/features/files/renderers/MonacoMergeRenderer.test.tsx src/features/files/FilesSurface.test.tsx src/features/workbench/coreSurfaceRegistry.test.ts`

Expected: Draft remains disabled and the close guard cannot distinguish outcomes.

- [ ] **Step 3: Implement revision-safe draft editing**

Use an editable Monaco model keyed by `draft_id`, save after a 300 ms idle debounce with expected revision, flush on mode change/close, and display backend conflicts without fabricating markers. A stale save reloads and offers compare/retry; it does not overwrite another presentation's newer draft.

- [ ] **Step 4: Wire independent Apply**

Apply invokes only `apply_draft`. On success, keep or clear the draft according to whether draft text equals the applied file; update working revision through the normal watcher event. Do not invoke prompt delivery or mark review sent.

- [ ] **Step 5: Replace the Files close prompt with three outcomes**

Extend the injected Files dirty prompt to return `keep_draft`, `discard_draft`, or `cancel`. Keep flushes the durable draft and allows close; Discard invokes backend discard then allows; Cancel rejects. Other surface close guards retain their current two-state contract.

- [ ] **Step 6: Run focused tests and lint**

Run: `npm run test -- --run src/features/files src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/navigationService.test.ts`

Run: `npm run lint`

Expected: Draft pinning, save, apply, conflict, dirty badge, and close outcomes pass.

- [ ] **Step 7: Commit Draft UI**

```bash
git add src/features/files src/features/workbench
git commit -m "feat(files): add draft and merge workflow"
```

### Task 6: Review schemas, annotations, and explicit comment lifecycle

**Files:**
- Create: `crates/wardian-core/src/artifacts/review.rs`
- Modify: `crates/wardian-core/src/artifacts/mod.rs`
- Modify: `crates/wardian-core/src/artifacts/models.rs`
- Modify: `crates/wardian-core/src/artifacts/store.rs`
- Modify: `src-tauri/src/commands/artifacts.rs`
- Create: `src-tauri/src/commands/reviews.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `crates/wardian-core/src/artifacts/review.rs`
- Test: `src-tauri/src/commands/reviews.rs`

**Interfaces:**
- Consumes: artifact versions, drafts, checkpoint IDs, content hashes, normalized intrinsic coordinates, and addressed IDs from Plan 2 presentation.
- Produces: `ReviewAnchorV1`, `ReviewCommentV1`, `ReviewCommentState`, `ArtifactReviewV1`, `create_review`, `resolve_review_comment`, `approve_artifact`, and deterministic anchor carry-forward.

- [ ] **Step 1: Add failing schema and transition tests**

Cover valid/invalid line ranges, clamped image/PDF ratios, page bounds, general notes, open-to-addressed, addressed-to-resolved, direct user resolve, outdated anchors, unchanged exact-context carry-forward, invalid addressed IDs, approval, and no inferred resolution from changed text.

- [ ] **Step 2: Run focused review tests**

Run: `cargo test -p wardian-core artifacts::review -- --test-threads=1`

Expected: review types and transition functions are missing.

- [ ] **Step 3: Define review and anchor contracts**

`ArtifactReviewV1` contains review ID, artifact/version IDs, base/current/draft hashes, unified patch blob hash, structured comments, general note, apply state, prompt checkpoint reference, delivery state, and timestamps. Each comment stores its originating version ID and explicit state.

- [ ] **Step 4: Implement deterministic mapping only**

Carry a line anchor forward only when its exact context hash occurs once in the new version and relative selected text is identical. Carry spatial anchors only when the immutable underlying version is still selected. Otherwise retain the original anchor and mark it outdated. Never inspect semantic similarity.

- [ ] **Step 5: Apply agent-addressed claims on presentation**

When Plan 2 appends a version with `addressed_comment_ids`, validate each ID belongs to the same thread and transition open comments to `agent_marked_addressed`. Do not transition to `resolved`; only `resolve_review_comment` does that.

- [ ] **Step 6: Add approval and review persistence commands**

`create_review` persists before delivery; `approve_artifact` atomically updates thread status and Queue actionability; `resolve_review_comment` records user resolution. All commands return typed current state so UI does not guess transitions.

- [ ] **Step 7: Run core and Tauri review tests**

Run: `cargo test -p wardian-core artifacts::review -- --test-threads=1`

Run: `cargo test -p Wardian reviews -- --test-threads=1`

Expected: schemas, anchors, transition ownership, persistence, and approval tests pass.

- [ ] **Step 8: Commit review domain logic**

```bash
git add crates/wardian-core/src/artifacts src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(artifacts): add review and annotation model"
```

### Task 7: Responsive review drawer and annotation UI

**Files:**
- Create: `src/features/files/ReviewDrawer.tsx`
- Create: `src/features/files/ReviewDrawer.test.tsx`
- Create: `src/features/files/annotations/LineCommentController.ts`
- Create: `src/features/files/annotations/LineCommentController.test.ts`
- Create: `src/features/files/annotations/SpatialAnnotationOverlay.tsx`
- Create: `src/features/files/annotations/SpatialAnnotationOverlay.test.tsx`
- Modify: `src/features/files/renderers/MonacoTextRenderer.tsx`
- Modify: `src/features/files/renderers/MonacoDiffRenderer.tsx`
- Modify: `src/features/files/renderers/ImageRenderer.tsx`
- Modify: `src/features/files/renderers/PdfRenderer.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesSurface.css`
- Test: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: review/version DTOs, Monaco selections/decorations, image intrinsic dimensions, PDF page viewport transforms, and review commands.
- Produces: collapsible/overlay ReviewDrawer, line/range comments, image/PDF region annotations, general note, approval, and accessible comment navigation.

- [ ] **Step 1: Add failing responsive and annotation tests**

Assert a wide pane reserves drawer width, a narrow pane overlays without shrinking the renderer below minimum, Escape closes overlay, every spatial annotation appears in a keyboard list, Monaco ranges are 1-based DTOs, image/PDF ratios survive zoom/resize, comment states use sentence case, and Approve is separate from Send.

- [ ] **Step 2: Run focused UI tests**

Run: `npm run test -- --run src/features/files/ReviewDrawer.test.tsx src/features/files/annotations src/features/files/FilesSurface.test.tsx`

Expected: drawer and annotation modules are missing.

- [ ] **Step 3: Implement a pane-container-responsive drawer**

Use the Files pane container query, not viewport media queries. At or above 760px inline size, the drawer is a collapsible 320px side region. Below it, the drawer is a focus-trapped overlay with a scrim and restores focus on close. Keep Preview/Changes/Draft in the mode row, not in the drawer.

- [ ] **Step 4: Implement trusted anchors**

Monaco controller converts selections to version-bound line/column anchors and computes the context hash through the backend. Spatial overlay sits outside image/PDF content, records ratios relative to intrinsic dimensions/page viewport, and never injects annotation code into a renderer document.

- [ ] **Step 5: Render provenance, versions, comments, and state**

The drawer shows title/description, origin, canonical path, version list, open/addressed/resolved/outdated comments, general note, and approval status. Agent-marked addressed uses sentence case and requires a user Resolve action.

- [ ] **Step 6: Run focused tests and accessibility checks**

Run: `npm run test -- --run src/features/files/ReviewDrawer.test.tsx src/features/files/annotations src/features/files/FilesSurface.test.tsx`

Run: `npm run lint`

Expected: responsive, focus, coordinate, keyboard, and explicit-state tests pass.

- [ ] **Step 7: Commit review UI**

```bash
git add src/features/files
git commit -m "feat(files): add artifact review drawer"
```

### Task 8: Send review references through checkpointed user delivery

**Files:**
- Create: `src-tauri/src/artifact_review_service.rs`
- Modify: `src-tauri/src/commands/reviews.rs`
- Modify: `src-tauri/src/control.rs`
- Modify: `crates/wardian-core/src/control.rs`
- Modify: `crates/wardian-cli/src/live.rs`
- Modify: `crates/wardian-cli/src/artifact.rs`
- Test: `src-tauri/src/artifact_review_service.rs`
- Test: `crates/wardian-cli/tests/artifact_cli.rs`
- Modify: `src/features/files/ReviewDrawer.tsx`
- Modify: `src/features/files/FilesModeBar.tsx`
- Test: `src/features/files/ReviewDrawer.test.tsx`

**Interfaces:**
- Consumes: durable review, artifact origin session, Task 2 checkpointed user-delivery wrapper, `ArtifactReviewShow` control request, and current Queue item.
- Produces: `send_artifact_review`, concise attributed provider input, review retrieval JSON, `feedback_sent` lifecycle transition, and independent Apply/Send UI state.

- [ ] **Step 1: Add failing delivery-order and independence tests**

Assert review persist -> prompt checkpoint persist -> provider submit -> delivery outcome persist; message contains review/artifact IDs and retrieval command but not a large patch; send does not modify disk; apply does not send; failed delivery retains review; successful send transitions status/Queue; and `review show --latest` returns all structured fields.

- [ ] **Step 2: Run focused service/CLI/UI tests**

Run: `cargo test -p Wardian artifact_review_service -- --test-threads=1`

Run: `cargo test -p wardian-cli artifact -- --test-threads=1`

Run: `npm run test -- --run src/features/files/ReviewDrawer.test.tsx`

Expected: send service and lifecycle assertions fail.

- [ ] **Step 3: Persist before checkpoint and delivery**

Create an immutable review with patch/comments/note first. Invoke `submit_user_prompt_with_checkpoint` using `origin_kind: artifact_review`; update the review with the returned checkpoint ID before provider submission through the wrapper's pre-submit association hook. The provider message is:

```text
Wardian artifact review <review-id> is available for artifact <artifact-id>.
Run `wardian artifact review show <artifact-id> --review <review-id>` to inspect it.
```

Include title and general note only when the complete message remains under the existing inline input limit. Never paste the patch.

- [ ] **Step 4: Record delivery and lifecycle atomically**

On successful submit, set review delivery state and manifest status `feedback_sent`; mark Queue action handled. On failure, retain the review with failed delivery detail and offer Retry. Retry reuses the same review ID and creates a new delivery attempt, not a duplicate review.

- [ ] **Step 5: Complete review retrieval**

`ArtifactReviewShowResponseV1` returns base/current hashes, unified patch text or revision-bound blob reference according to size, comments, general note, apply state, checkpoint ID, and delivery attempts. Disk fallback remains read-only and schema-validated.

- [ ] **Step 6: Wire independent buttons**

Show Apply to file only for text drafts and Send to agent only for artifacts with an origin. Either may be used first. The UI displays separate applied and sent state; it never combines them into one ambiguous Save button.

- [ ] **Step 7: Run focused tests**

Run: `cargo test -p Wardian artifact_review_service -- --test-threads=1`

Run: `cargo test -p wardian-cli artifact -- --test-threads=1`

Run: `npm run test -- --run src/features/files/ReviewDrawer.test.tsx src/features/files/FilesSurface.test.tsx`

Expected: ordering, concise reference, retrieval, retry, and independence tests pass.

- [ ] **Step 8: Commit review delivery**

```bash
git add src-tauri/src/artifact_review_service.rs src-tauri/src/commands/reviews.rs src-tauri/src/control.rs crates/wardian-core/src/control.rs crates/wardian-cli src/features/files
git commit -m "feat(artifacts): deliver structured artifact reviews"
```

### Task 9: Native review/checkpoint proof and documentation

**Files:**
- Create: `e2e-native/tests/artifact-review-native.test.mjs`
- Modify: `e2e-native/tests/artifact-lifecycle-native.test.mjs`
- Create: `e2e/tests/files-review.spec.ts`
- Modify: `e2e/fixtures/workbenchIpcMock.ts`
- Modify: `docs/guide/artifacts.md`
- Modify: `docs/guide/explorer.md`
- Modify: `docs/guide/cli.md`
- Modify: `docs/developer/state-management.md`
- Modify: `docs/developer/tauri-command-reference.md`
- Modify: `docs/developer/ipc-events.md`

**Interfaces:**
- Consumes: native mock agent, real filesystem, real CLI/control endpoint, Files UI, and checkpoint/review stores.
- Produces: evidence for checkpoint ordering/diffs, restart drafts, merge/apply, review delivery/retrieval, addressed comment lifecycle, and responsive review UI.

- [ ] **Step 1: Add a failing native end-to-end review loop**

Start a mock agent, wait for index readiness, submit a prompt, change/add/delete/rename files, open Changes, draft a text edit, create line and image comments, send review, retrieve it with CLI, re-present with `--address`, resolve as user, apply the draft, restart, and verify all durable state.

- [ ] **Step 2: Add conflict and unavailable-baseline scenarios**

Create overlapping external edits and assert disk remains unchanged until conflict resolution. Submit a prompt before initial indexing completes and assert the UI says `Prompt baseline unavailable` rather than displaying a mislabeled diff.

- [ ] **Step 3: Run the native test**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-review-native.test.mjs`

Expected before final wiring: the first checkpoint/draft/review command fails.

- [ ] **Step 4: Add browser review interactions**

Mock descriptors and review commands to prove mode changes, Monaco diff state, transient pinning, independent actions, wide/overlay drawer, keyboard comment list, explicit addressed/resolved states, and conflict UI. Do not claim real merge or delivery behavior.

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/files-review.spec.ts`

Expected: all browser interaction assertions pass.

- [ ] **Step 5: Document the review contract**

Explain Changes since prompt semantics and authorship caveat, index readiness, Draft persistence, Apply versus Send, conflicts, comment anchors/states, approval, CLI retrieval, and agent `--address`. Use cross-platform paths and commands.

- [ ] **Step 6: Run the slice verification suite**

Run: `npm run lint`

Run: `npm run test`

Run: `npm run build`

Run: `npm run test:e2e -- e2e/tests/files-review.spec.ts`

Run: `cargo check --workspace`

Run: `cargo clippy --workspace -- -D warnings`

Run: `cargo test --workspace -- --test-threads=1`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-review-native.test.mjs`

Expected: every command passes and Files remains reserved.

- [ ] **Step 7: Capture PR evidence and commit tests/docs**

Capture Changes plus the review drawer and a conflict state under `e2e/screenshots/files-surface/<timestamp>/`, upload at least one representative image, and embed its HTTPS URL in the PR body.

```bash
git add e2e-native e2e docs/guide docs/developer
git commit -m "test(artifacts): prove prompt scoped review flow"
```
