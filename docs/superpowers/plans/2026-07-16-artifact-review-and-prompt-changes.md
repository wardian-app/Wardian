# Artifact Review and Prompt Changes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add trustworthy prompt/presented-version baselines, line/spatial review, approval, and structured agent feedback to the conventional artifact-enabled Files editor without replacing its shared buffer or explicit Save workflow.

**Architecture:** A shared backend `PromptChangeTracker` maintains content-addressed indexes for each agent's authorized roots and captures durable checkpoints immediately before user-originated delivery. Exact prompt and presented-version adapters feed the shipped `FileComparisonLens`, while the existing canonical `FileEditorController` remains the only working buffer and durable recovery owner. A backend review service freezes immutable submission snapshots, stores anchored comments and approval state, and sends concise review references. React adds a responsive review drawer and historical baseline choices; it never creates another editor mode or buffer or performs authoritative anchoring and comment-state transitions.

**Tech Stack:** Rust, notify, SHA-256, diffy, serde, Tauri delivery services, Monaco Diff Editor, React 19, Zustand, Vitest, Playwright, and native E2E.

## Global Constraints

- Plans 1 and 2 must be merged first. Reuse their authorization, stable revisions, blob store, artifact origin, and Files resource controller.
- Label temporal diffs **Changes since prompt**. Never label them “Agent changes” or claim authorship.
- Capture a checkpoint immediately before Wardian delivers a user-originated prompt. Inter-agent control messages, terminal escape replies, and provider output do not advance it.
- Prompt delivery must not synchronously crawl a workspace. If the incremental index is not ready, deliver with a durable `baseline_unavailable` association.
- Index `AgentConfig.folder` and `include_directories`; exclude `system_include_directories`, Wardian state, VCS internals, and default high-churn caches.
- The shipped Files editor owns one durable shared buffer, explicit Save, stale Merge/Reload/Cancel, and final-close Save/Don't Save/Cancel. This plan must not add a draft service, Apply operation, or second Monaco model.
- Save and Send are independent operations. Save writes the shared buffer through the editor core; Send never writes the file or clears dirty state.
- Send freezes one immutable snapshot of the current shared buffer, selected exact baseline, patch, general note, and included comments. Retries reuse the same review ID and payload.
- Comments created while a Send is in flight remain queued for the next submission and are never silently folded into the in-flight snapshot.
- `artifact:<id>` remains a provenance/presentation key. Its attachment adapter resolves to the canonical file/blob controller; review UI consumes that controller and never owns artifact text.
- Save As creates an ordinary file presentation and never retargets an artifact thread or its review provenance.
- This plan owns prompt and presented-version baseline adapters, comments, Send, retrieval, addressed-state transitions, and approval. The lifecycle plan owns artifact persistence/CLI/provenance/attention; the live-isolation plan owns HTML/SVG sandboxing and final Files activation.
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

### Task 3: Prompt and presented-version baseline adapters

**Files:**
- Create: `src-tauri/src/commands/changes.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/types/files.ts`
- Modify: `src/features/files/fileResourceClient.ts`
- Create: `src/features/files/usePromptChanges.ts`
- Create: `src/features/files/usePromptChanges.test.tsx`
- Modify: `src/features/files/fileDiffModel.ts`
- Modify: `src/features/files/fileDiffModel.test.ts`
- Modify: `src/features/files/FileComparisonLens.tsx`
- Modify: `src/features/files/FileComparisonLens.test.tsx`
- Create: `src/features/files/renderers/BinaryVersionCompare.tsx`
- Create: `src/features/files/renderers/BinaryVersionCompare.test.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesHeader.tsx`
- Modify: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: `PromptChangeTracker`, selected agent/checkpoint context, artifact selected version, the shared `FileEditorController`, shipped Saved-file comparison variants, Monaco diff limits, and resource tickets.
- Produces: `get_file_prompt_changes`, `FilePromptChangesV1`, exact prompt/presented-version `FileDiffBaseline` adapters, baseline-unavailable UI, and image/PDF previous/current comparison.

- [ ] **Step 1: Add failing API and renderer tests**

Cover modified/added/deleted/renamed text paths, current agent context, explicitly selected checkpoint, selected artifact version, no context, unavailable baseline, stale checkpoint, line limits, image/PDF metadata, and exact **Changes since prompt** labeling. Assert the adapter does not replace the shared Monaco model and keeps `buffer_base_hash`, `disk_head_hash`, and `review_base_hash` distinct.

```ts
expect(screen.getByRole("button", { name: "Compare against Changes since prompt" })).toBeEnabled();
expect(screen.queryByText(/agent changes/i)).not.toBeInTheDocument();
```

- [ ] **Step 2: Run focused tests and confirm historical baselines remain unavailable**

Run: `npm run test -- --run src/features/files/usePromptChanges.test.tsx src/features/files/fileDiffModel.test.ts src/features/files/FileComparisonLens.test.tsx src/features/files/renderers/BinaryVersionCompare.test.tsx src/features/files/FilesSurface.test.tsx`

Expected: missing adapter modules and historical-baseline assertions fail while Saved-file comparison remains green.

- [ ] **Step 3: Add a typed comparison command**

`get_file_prompt_changes` accepts resource key, canonical path, `agent_id`, optional `checkpoint_id`, and current revision. It returns baseline state, path change kind, old/new descriptors, revision-bound read sources, checkpoint metadata, and a precise unavailable reason. A presented-version adapter resolves only the exact selected artifact blob. Neither adapter substitutes Saved-file, first-observed, or another presented revision under a historical label.

- [ ] **Step 4: Implement Monaco line comparison**

Extend `fileDiffModel` and `FileComparisonLens` with prompt-checkpoint and presented-version baseline variants. Load the immutable baseline lazily, keep the current side bound to the existing shared controller, enforce 5 MiB/100,000-line limits per side, and dispose baseline models by reference count. Added/deleted files use an empty counterpart and a rename displays both paths. Comparison remains read-only and never changes editor presentation or buffer ownership.

- [ ] **Step 5: Implement binary version comparison**

For image/PDF artifacts, the comparison lens shows the selected exact baseline and current working state side by side with dimensions/page count, byte size, hashes, and timestamps. Do not label it a semantic visual diff. Ordinary binary files without a checkpoint-readable old blob show a typed unavailable state.

- [ ] **Step 6: Expose only honest baseline choices**

Files presentation state may remember the selected baseline variant, but an unavailable historical source stays visible as a typed unavailable comparison. It never falls back to Saved-file or a current rendering while retaining the historical label. HTML/SVG source comparison becomes eligible only after the live-isolation plan activates those renderers; the comparison still uses inert source, not the live document.

- [ ] **Step 7: Run focused tests**

Run: `npm run test -- --run src/features/files/usePromptChanges.test.tsx src/features/files/fileDiffModel.test.ts src/features/files/FileComparisonLens.test.tsx src/features/files/renderers/BinaryVersionCompare.test.tsx src/features/files/FilesSurface.test.tsx`

Run: `cargo test -p Wardian changes -- --test-threads=1`

Expected: scope, labels, unavailable states, limits, and comparison tests pass.

- [ ] **Step 8: Commit historical baseline adapters**

```bash
git add src-tauri/src/commands src-tauri/src/lib.rs src/types/files.ts src/features/files
git commit -m "feat(files): add historical review baselines"
```

### Task 4: Immutable review submission snapshots

**Files:**
- Create: `crates/wardian-core/src/artifacts/review_snapshot.rs`
- Modify: `crates/wardian-core/src/artifacts/mod.rs`
- Modify: `crates/wardian-core/src/artifacts/models.rs`
- Modify: `crates/wardian-core/src/artifacts/store.rs`
- Create: `src-tauri/src/commands/review_snapshots.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `crates/wardian-core/src/artifacts/review_snapshot.rs`
- Test: `src-tauri/src/commands/review_snapshots.rs`

**Interfaces:**
- Consumes: artifact blob store, the current shared-buffer bytes plus observed controller generation, distinct buffer/disk/review hashes, exact prompt/version references, and queued comment IDs.
- Produces: `ReviewSnapshotV1`, `freeze_review_snapshot`, immutable patch/body blob references, and idempotent snapshot lookup by review ID.

- [ ] **Step 1: Add failing snapshot and idempotency tests**

Cover unsaved and saved shared-buffer text, exact baseline identity, distinct `buffer_base_hash`/`disk_head_hash`/`review_base_hash`, stale controller generation, restart survival, immutable patch/body blobs, one payload per review ID, byte/line limits, unauthorized resources, and a retry returning the original payload after the editor or comment queue changes.

```rust
#[test]
fn retry_keeps_the_frozen_payload() {
    let first = store
        .freeze_review_snapshot(request("review-1", "buffer-a"))
        .unwrap();
    let retry = store
        .freeze_review_snapshot(request("review-1", "buffer-b"))
        .unwrap();
    assert_eq!(first.payload_hash, retry.payload_hash);
}
```

- [ ] **Step 2: Run focused tests and confirm snapshot APIs are missing**

Run: `cargo test -p wardian-core artifacts:: -- --test-threads=1`

Run: `cargo test -p Wardian review_snapshots -- --test-threads=1`

Expected: tests fail because review snapshot/store APIs are missing.

- [ ] **Step 3: Persist immutable review snapshots**

Store one atomic record per review ID containing canonical resource identity, artifact/version/checkpoint references, the three distinct hashes, frozen working-buffer blob, derived patch blob, included comment IDs, general note, and created timestamp. Deduplicate immutable blobs by content hash. The record contains no writable editor state and never becomes a recovery source.

- [ ] **Step 4: Freeze exactly one Send payload**

`freeze_review_snapshot` validates canonical resource identity, observed controller generation, exact baseline reference, and included comment revisions before storing. A repeated review ID returns the original snapshot and does not observe later buffer or comment changes. New comments remain queued outside that snapshot.

- [ ] **Step 5: Keep persistence out of review snapshots**

Do not add apply, merge, file-write, or draft commands. The Files editor core remains the only owner of explicit Save, atomic replacement, stale Merge/Reload/Cancel, and durable recovery. Send may snapshot dirty buffer text without saving it.

- [ ] **Step 6: Expose typed snapshot commands**

Commands take `review_id`, `resource_key`, observed controller generation, exact baseline reference, working-buffer bytes or immutable blob reference, and included comment revisions. Return typed stale-generation, baseline-unavailable, authorization, and size errors without mutating disk.

- [ ] **Step 7: Run core and Tauri tests**

Run: `cargo test -p wardian-core artifacts:: -- --test-threads=1`

Run: `cargo test -p Wardian review_snapshots -- --test-threads=1`

Expected: persistence, restart, exact-baseline, immutable-payload, authorization, and retry-idempotency tests pass.

- [ ] **Step 8: Commit review snapshot services**

```bash
git add crates/wardian-core src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(artifacts): freeze review submission snapshots"
```

### Task 5: Attach review state to the shared editor controller

**Files:**
- Create: `src/features/files/useReviewWorkingSet.ts`
- Create: `src/features/files/useReviewWorkingSet.test.tsx`
- Modify: `src/features/files/fileEditorController.ts`
- Modify: `src/features/files/fileEditorController.test.ts`
- Modify: `src/features/files/FileComparisonLens.tsx`
- Modify: `src/features/files/FileComparisonLens.test.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesHeader.tsx`
- Modify: `src/features/files/filesPresentationStore.ts`
- Test: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: Task 3 baseline adapters, Task 4 snapshot command, the canonical `FileEditorController`, current Saved-file annotations, and Files presentation state.
- Produces: review working-set metadata over the shared buffer, historical baseline selection, queued-snapshot state, and unchanged editor-core Save/close behavior.

- [ ] **Step 1: Add failing shared-controller review tests**

Assert a review opened from `artifact:<id>` attaches to the same canonical controller as an ordinary file presentation; edits, undo, dirty dots, Saved-file annotations, durable recovery, and stale state remain shared; selecting a historical baseline does not replace the working model; Send can snapshot unsaved text; Save As opens an ordinary file and leaves the artifact/review target unchanged; and no additional editor mode or buffer is created.

- [ ] **Step 2: Run focused frontend tests**

Run: `npm run test -- --run src/features/files/useReviewWorkingSet.test.tsx src/features/files/fileEditorController.test.ts src/features/files/FileComparisonLens.test.tsx src/features/files/FilesSurface.test.tsx`

Expected: missing review working-set wiring fails while existing editor core tests stay green.

- [ ] **Step 3: Implement a metadata-only review working set**

Track selected baseline, included comment revisions, general note, and pending/in-flight review IDs. Read working text and generation from the canonical controller only when freezing a snapshot. Do not copy text into review state, create another Monaco URI, or debounce a review-owned persistence path.

- [ ] **Step 4: Preserve independent Save and Send**

Save continues to invoke only the editor core's guarded write and does not mark a review sent. Preparing or sending a review invokes no file write, does not clear dirty state, and does not advance `buffer_base_hash` or `disk_head_hash`.

- [ ] **Step 5: Preserve the Files close and stale contracts**

The final dirty presentation still asks **Save**, **Don't Save**, or **Cancel**. A stale dirty buffer still offers **Merge**, **Reload from disk**, or **Cancel**. Review metadata never changes those choices and is retained independently according to the review store.

- [ ] **Step 6: Run focused tests and lint**

Run: `npm run test -- --run src/features/files src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/navigationService.test.ts`

Run: `npm run lint`

Expected: shared editing, independent Save/Send, historical baseline selection, dirty badges, recovery, and unchanged close/stale outcomes pass.

- [ ] **Step 7: Commit shared-controller review wiring**

```bash
git add src/features/files src/features/workbench
git commit -m "feat(files): attach reviews to shared editor state"
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
- Consumes: artifact versions, immutable Task 4 review snapshots, checkpoint IDs, content hashes, normalized intrinsic coordinates, and addressed IDs from Plan 2 presentation.
- Produces: `ReviewAnchorV1`, `ReviewCommentV1`, `ReviewCommentState`, `ArtifactReviewV1`, `create_review`, `resolve_review_comment`, `approve_artifact`, and deterministic anchor carry-forward.

- [ ] **Step 1: Add failing schema and transition tests**

Cover valid/invalid line ranges, clamped image/PDF ratios, page bounds, general notes, open-to-addressed, addressed-to-resolved, direct user resolve, outdated anchors, unchanged exact-context carry-forward, invalid addressed IDs, approval, and no inferred resolution from changed text.

- [ ] **Step 2: Run focused review tests**

Run: `cargo test -p wardian-core artifacts::review -- --test-threads=1`

Expected: review types and transition functions are missing.

- [ ] **Step 3: Define review and anchor contracts**

`ArtifactReviewV1` contains review ID, artifact/version IDs, distinct buffer-base/disk-head/review-base hashes, frozen working-buffer and unified-patch blob hashes, structured included-comment revisions, general note, prompt checkpoint reference, delivery state, and timestamps. It records no apply state and owns no editable draft. Each comment stores its originating version ID and explicit state.

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
- Modify: `src/features/files/FileComparisonLens.tsx`
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

Use the Files pane container query, not viewport media queries. At or above 760px inline size, the drawer is a collapsible 320px side region. Below it, the drawer is a focus-trapped overlay with a scrim and restores focus on close. Keep the Book/Pencil current-state control, comparison button, annotations, and diff count in their existing Files locations; do not add Preview/Changes/Draft modes.

When the drawer is inline, subtract its occupied width before resolving the comparison lens. The existing lens remains side by side at 720 px or wider and unified below 720 px. A forced side-by-side preference is honored to the 560 px hard minimum, then receives only a temporary unified override.

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
- Modify: `src/features/files/FilesHeader.tsx`
- Test: `src/features/files/ReviewDrawer.test.tsx`

**Interfaces:**
- Consumes: immutable review snapshot, artifact origin session, Task 2 checkpointed user-delivery wrapper, `ArtifactReviewShow` control request, current Queue item, and queued comment revisions.
- Produces: `send_artifact_review`, concise attributed provider input, review retrieval JSON, `feedback_sent` lifecycle transition, and independent Save/Send UI state.

- [ ] **Step 1: Add failing delivery-order and independence tests**

Assert snapshot/review persist -> prompt checkpoint persist -> provider submit -> delivery outcome persist; message contains review/artifact IDs and retrieval command but not a large patch; Send does not modify disk or clear dirty state; Save does not send; a failed delivery retains the frozen review; Retry reuses the same review ID and payload; comments added in flight remain queued; successful Send transitions status/Queue; and `review show --latest` returns all structured fields.

- [ ] **Step 2: Run focused service/CLI/UI tests**

Run: `cargo test -p Wardian artifact_review_service -- --test-threads=1`

Run: `cargo test -p wardian-cli artifact -- --test-threads=1`

Run: `npm run test -- --run src/features/files/ReviewDrawer.test.tsx`

Expected: send service and lifecycle assertions fail.

- [ ] **Step 3: Persist before checkpoint and delivery**

Freeze an immutable review with the current shared-buffer bytes, exact baseline, patch, included comments, and note first. Invoke `submit_user_prompt_with_checkpoint` using `origin_kind: artifact_review`; update the review with the returned checkpoint ID before provider submission through the wrapper's pre-submit association hook. The provider message is:

```text
Wardian artifact review <review-id> is available for artifact <artifact-id>.
Run `wardian artifact review show <artifact-id> --review <review-id>` to inspect it.
```

Include title and general note only when the complete message remains under the existing inline input limit. Never paste the patch.

- [ ] **Step 4: Record delivery and lifecycle atomically**

On successful submit, set review delivery state and manifest status `feedback_sent`; mark Queue action handled. On failure, retain the review with failed delivery detail and offer Retry. Retry reuses the same review ID, frozen payload, and included-comment set while creating a new delivery attempt. Comments created after freezing remain queued for a future review.

- [ ] **Step 5: Complete review retrieval**

`ArtifactReviewShowResponseV1` returns the three distinct hashes, frozen working-buffer reference, unified patch text or revision-bound blob reference according to size, included comments, general note, checkpoint ID, and delivery attempts. Disk fallback remains read-only and schema-validated.

- [ ] **Step 6: Wire independent Save and Send actions**

Keep **Save** in the conventional Files action and show **Send to agent** only for artifacts with an origin. Either may be used first. Send freezes the current shared buffer and selected exact baseline without invoking Save. The UI displays dirty/saved state separately from queued/in-flight/sent review state and never adds Apply or a review-owned Save action.

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
- Produces: evidence for checkpoint ordering and exact historical diffs, shared-buffer review snapshots, independent Save/Send, review delivery/retrieval, addressed comment lifecycle, and responsive review UI.

- [ ] **Step 1: Add a failing native end-to-end review loop**

Start a mock agent, wait for index readiness, submit a prompt, change/add/delete/rename files, select **Changes since prompt** in the comparison lens, edit the shared buffer, create line and image comments, Send without saving, retrieve the frozen review with CLI, Save independently, re-present with `--address`, resolve as user, restart, and verify recovery, comments, and review state. Also prove that a duplicate ordinary-file presentation and `artifact:<id>` presentation share one buffer and that Save As opens an ordinary file without retargeting the artifact.

- [ ] **Step 2: Add conflict and unavailable-baseline scenarios**

Create an external edit while the shared buffer is dirty and assert the exact **Merge**, **Reload from disk**, and **Cancel** stale choices. Merge must remain dirty until explicit Save; Reload must discard recovery and adopt disk; Cancel must preserve stale state. Submit a prompt before initial indexing completes and assert the UI says `Prompt baseline unavailable` rather than displaying a mislabeled diff. Request an unavailable presented version and prove Wardian does not substitute Saved-file or another version.

- [ ] **Step 3: Run the native test**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-review-native.test.mjs`

Expected before final wiring: the first checkpoint/baseline/review-snapshot command fails.

- [ ] **Step 4: Add browser review interactions**

Mock descriptors and review commands to prove baseline selection, inline annotations and diff count, Book/Pencil current state, independent Save/Send actions, wide/overlay drawer, keyboard comment list, explicit addressed/resolved states, and stale-choice UI. Prove side-by-side at 720 px, unified below 720 px, forced side-by-side to 560 px, and drawer-width subtraction. Do not claim native authorization, filesystem atomicity, durable restart recovery, real watcher conflict behavior, or delivery transport from browser coverage.

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/files-review.spec.ts`

Expected: all browser interaction assertions pass.

- [ ] **Step 5: Document the review contract**

Explain Changes since prompt semantics and authorship caveat, index readiness, exact historical-baseline availability, shared durable editor recovery, explicit Save versus Send, stale Merge/Reload/Cancel, comment anchors/states, approval, CLI retrieval, and agent `--address`. Document that `artifact:<id>` attaches to the canonical file/blob controller, that one buffer is shared, and that Save As never retargets the artifact. Use cross-platform paths and commands.

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

Capture the historical comparison lens with inline annotations/diff count, the review drawer, and a stale-buffer state under `e2e/screenshots/files-surface/<timestamp>/`, upload at least one representative image, and embed its HTTPS URL in the PR body.

```bash
git add e2e-native e2e docs/guide docs/developer
git commit -m "test(artifacts): prove prompt scoped review flow"
```
