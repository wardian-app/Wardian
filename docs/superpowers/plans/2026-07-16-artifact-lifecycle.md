# Artifact Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an authorized Wardian agent present a local file as a durable, versioned artifact thread that opens in a background Files tab, remains discoverable after close or restart, and can be queried through provider-neutral CLI commands.

**Architecture:** `wardian-core` owns inspectable artifact schemas, immutable content-addressed blobs, reuse rules, and atomic store transactions. The desktop control server resolves the session origin, captures a stable file revision, persists before emitting an acknowledged presentation event, and returns structured JSON only after background routing completes. React treats artifact identity as `artifact:<artifact_id>` and adds versions, provenance, attention, Queue, and Quick Open without changing ordinary file identity.

**Tech Stack:** Rust, serde, SHA-256, Tauri control endpoint/events, clap, React 19, Zustand, Workbench NavigationService, Vitest, Playwright, and native E2E.

## Global Constraints

- Plan 1 must be merged first; reuse its `AuthorizedRootService`, file descriptors, stable revisions, and resource tickets.
- Artifact keys are `artifact:<artifact_id>`, never `file:<path>` and never the manifest path.
- `wardian artifact present` requires a running desktop app using the same `WARDIAN_HOME` and a nonempty `WARDIAN_SESSION_ID`.
- Presentation persists the immutable version and manifest before UI routing. UI routing is acknowledged before CLI success.
- A presentation never focuses a tab or changes the active group. It opens in the active group as a permanent background tab and requests attention.
- Automatic reuse is only for the same resolved origin session and canonical path among active threads. `--new` and `--artifact` are mutually exclusive overrides.
- File writes and watcher events do not create artifact versions. Only an explicit present request does.
- A closed tab does not delete the thread, version, attention item, or recent entry.
- All persistence is versioned, snake_case, content-addressed, and atomic. No file bytes enter Workbench state or Queue state.
- The Files launcher card remains reserved through this plan.

---

### Task 1: Versioned artifact models and atomic content-addressed store

**Files:**
- Modify: `crates/wardian-core/src/lib.rs`
- Create: `crates/wardian-core/src/artifacts/mod.rs`
- Create: `crates/wardian-core/src/artifacts/models.rs`
- Create: `crates/wardian-core/src/artifacts/store.rs`
- Modify: `crates/wardian-core/src/atomic_file.rs`
- Test: `crates/wardian-core/src/artifacts/models.rs`
- Test: `crates/wardian-core/src/artifacts/store.rs`

**Interfaces:**
- Consumes: Wardian home path, `atomic_file` last-known-good helpers, canonical path strings, SHA-256 hashes, and stable file bytes from Plan 1.
- Produces: `ArtifactManifestV1`, `ArtifactVersionV1`, `ArtifactOriginV1`, `ArtifactReviewStatus`, `ArtifactIndexV1`, `ArtifactStore`, `ArtifactStoreError`, and immutable blobs under `<WARDIAN_HOME>/artifacts`.

- [ ] **Step 1: Add failing schema, recovery, and deduplication tests**

Round-trip every schema; reject unknown schema versions and malformed references; persist two versions with identical bytes and assert one blob; interrupt a manifest write and recover the last-known-good manifest; rebuild a stale index from manifests; and reject path traversal in every artifact/review/checkpoint ID.

```rust
#[test]
fn identical_presentations_share_one_blob() {
    let store = ArtifactStore::open(test_artifact_root()).unwrap();
    let first = store.append_version(new_thread_input(b"same bytes")).unwrap();
    let second = store.append_version(existing_thread_input(first.artifact_id, b"same bytes")).unwrap();
    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(store.blob_count().unwrap(), 1);
}
```

- [ ] **Step 2: Run focused tests and confirm the artifact module is missing**

Run: `cargo test -p wardian-core artifacts:: -- --test-threads=1`

Expected: compilation fails because `wardian_core::artifacts` is not defined.

- [ ] **Step 3: Define explicit v1 schemas**

Use these required fields:

```rust
pub struct ArtifactOriginV1 {
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub provider: String,
}

pub struct ArtifactVersionV1 {
    pub version_id: String,
    pub sequence: u64,
    pub content_hash: String,
    pub size_bytes: u64,
    pub presented_at_ms: u64,
    pub addressed_comment_ids: Vec<String>,
}

pub struct ArtifactManifestV1 {
    pub schema: u8,
    pub artifact_id: String,
    pub canonical_path: String,
    pub title: String,
    pub description: Option<String>,
    pub origin: ArtifactOriginV1,
    pub status: ArtifactReviewStatus,
    pub active: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub versions: Vec<ArtifactVersionV1>,
    pub latest_review_id: Option<String>,
}
```

`ArtifactReviewStatus` serializes `presented`, `feedback_sent`, `updated`, and `approved`. An agent presentation after `feedback_sent` records `updated`; otherwise a new thread starts `presented`.

- [ ] **Step 4: Implement the inspectable store layout**

Create exactly:

```text
artifacts/index.json
artifacts/threads/<artifact-id>/manifest.json
artifacts/reviews/<review-id>.json
artifacts/checkpoints/<checkpoint-id>.json
artifacts/blobs/<sha256>
```

Write a blob with create-new semantics, flush it, then atomically update the manifest, then atomically update the index. If the index update fails, the manifest remains authoritative and the next open repairs the index. Validate all references and hashes while loading; quarantine no data silently.

- [ ] **Step 5: Expose store queries required by later tasks**

Implement `create_thread`, `append_version`, `load_manifest`, `load_version`, `list_recent`, `find_active_thread(origin_session_id, canonical_path)`, `set_attention`, and `close_thread`. `close_thread` marks `active: false`; it does not delete content.

- [ ] **Step 6: Run focused and crate tests**

Run: `cargo test -p wardian-core artifacts:: -- --test-threads=1`

Run: `cargo test -p wardian-core -- --test-threads=1`

Expected: schema, atomic recovery, repair, path safety, deduplication, and lifecycle tests pass.

- [ ] **Step 7: Commit the artifact store**

```bash
git add crates/wardian-core/src/lib.rs crates/wardian-core/src/atomic_file.rs crates/wardian-core/src/artifacts
git commit -m "feat(artifacts): add durable versioned store"
```

### Task 2: Stable presentation service and thread reuse rules

**Files:**
- Create: `src-tauri/src/state/artifact_runtime.rs`
- Modify: `src-tauri/src/state/mod.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Create: `src-tauri/src/artifact_service.rs`
- Test: `src-tauri/src/artifact_service.rs`
- Test: `src-tauri/src/state/artifact_runtime.rs`

**Interfaces:**
- Consumes: `ArtifactStore`, Plan 1 `FileResourceRuntime`, `AuthorizedRootService`, live agent/session state, and stable file revisions.
- Produces: `ArtifactService::present`, `ArtifactPresentationRequestV1`, `ArtifactPresentationResultV1`, `ArtifactShowResultV1`, and pending UI acknowledgements in `ArtifactRuntime`.

- [ ] **Step 1: Add failing service tests**

Cover same-origin/same-path reuse, different origins creating different threads, `force_new`, explicit target thread, explicit target origin/path mismatch, unreadable and unstable files, addressed comment IDs preserved, no watcher-created versions, persistence before event emission, and a timed-out UI acknowledgement returning a typed failure without rolling back persisted data.

```rust
#[tokio::test]
async fn same_origin_and_path_reuse_active_thread() {
    let first = service.present(request("session-a", "report.md")).await.unwrap();
    let second = service.present(request("session-a", "report.md")).await.unwrap();
    assert_eq!(first.artifact_id, second.artifact_id);
    assert!(second.reused_thread);
    assert_ne!(first.version_id, second.version_id);
}
```

- [ ] **Step 2: Run focused tests and confirm the service is missing**

Run: `cargo test -p Wardian artifact_service -- --test-threads=1`

Expected: compilation fails because `ArtifactService` and runtime state do not exist.

- [ ] **Step 3: Implement stable capture and authorization**

Resolve the origin from the active session, load its `AgentConfig`, authorize through the shared `AuthorizedRootService`, then request a stable Plan 1 revision. Read and hash the revision, recheck metadata/hash before commit, and retry for at most 2 seconds. Return `unstable_file_timeout` rather than snapshotting mid-write.

- [ ] **Step 4: Implement deterministic reuse**

The service follows this order:

1. Reject a request containing both `artifact_id` and `force_new`.
2. For `artifact_id`, load it and require the same origin session and canonical path.
3. For `force_new`, create a thread.
4. Otherwise reuse `find_active_thread(origin.session_id, canonical_path)` or create a thread.

Generate a new immutable version for every successful explicit call, even when the hash matches the preceding version. Blob deduplication still avoids duplicate bytes.

- [ ] **Step 5: Add an acknowledged presentation runtime**

`ArtifactRuntime` owns pending `presentation_id -> oneshot::Sender<ArtifactPresentationAckV1>` entries. After store commit, `ArtifactService` emits `artifact://presented` and waits up to 2 seconds for `ack_artifact_presentation`. The result distinguishes `persistence_state: "persisted"` from `ui_delivery_state: "routed_background"`. A timeout returns `ui_delivery_failed` to the CLI while retaining the durable thread for recovery/recent discovery.

- [ ] **Step 6: Run focused tests**

Run: `cargo test -p Wardian artifact_service -- --test-threads=1`

Expected: reuse, stable capture, authorization, persistence ordering, and acknowledgement tests pass.

- [ ] **Step 7: Commit the presentation service**

```bash
git add src-tauri/src/artifact_service.rs src-tauri/src/state
git commit -m "feat(artifacts): add presentation service"
```

### Task 3: Provider-neutral artifact CLI and control protocol

**Files:**
- Modify: `crates/wardian-core/src/control.rs`
- Modify: `crates/wardian-cli/src/args.rs`
- Modify: `crates/wardian-cli/src/live.rs`
- Modify: `crates/wardian-cli/src/main.rs`
- Modify: `crates/wardian-cli/src/errors.rs`
- Create: `crates/wardian-cli/src/artifact.rs`
- Test: `crates/wardian-cli/src/args.rs`
- Test: `crates/wardian-cli/src/artifact.rs`
- Test: `crates/wardian-cli/tests/artifact_cli.rs`

**Interfaces:**
- Consumes: the existing newline-delimited control endpoint, `WARDIAN_HOME`, `WARDIAN_SESSION_ID`, and Task 2 service DTOs.
- Produces: `ControlRequest::{ArtifactPresent, ArtifactShow, ArtifactReviewShow}`, `Command::Artifact`, `ArtifactCommand`, structured success JSON, and stable error codes.

- [ ] **Step 1: Add failing parser and wire-schema tests**

Test every documented command, repeated `--address`, `--new`/`--artifact` conflict, missing session ID for present, JSON request shape, nonzero app-not-running behavior, and pretty structured success output.

```rust
let cli = Cli::try_parse_from([
    "wardian", "artifact", "present", "report.md",
    "--title", "Report", "--address", "comment-1", "--address", "comment-2",
]).unwrap();
assert!(matches!(cli.command, Command::Artifact(_)));
```

- [ ] **Step 2: Run focused CLI tests and confirm `artifact` is unknown**

Run: `cargo test -p wardian-cli artifact -- --test-threads=1`

Expected: clap rejects `artifact` and control variants are missing.

- [ ] **Step 3: Add exact clap commands**

Implement:

```text
wardian artifact present <path> [--title <title>] [--description <markdown>]
  [--artifact <artifact-id> | --new] [--address <comment-id>]...
wardian artifact show <artifact-id> [--version <version-id>]
wardian artifact review show <artifact-id> [--review <review-id> | --latest]
```

Use a clap `ArgGroup` to reject `--artifact` with `--new` and `--review` with `--latest`. `present` resolves no path locally beyond making the input absolute; the desktop remains authoritative for canonicalization and authorization.

- [ ] **Step 4: Add control DTOs and public live-client functions**

Add `ArtifactPresent`, `ArtifactShow`, and `ArtifactReviewShow` variants to `ControlRequest`. The present variant carries `origin: MessageOrigin::WardianAgent { session_id }`; the CLI derives it only from `WARDIAN_SESSION_ID`. Add `live::artifact_present`, `live::artifact_show`, and `live::artifact_review_show` using the mutation timeout.

- [ ] **Step 5: Map errors without false success**

Stable CLI error codes are `app_not_running`, `invalid_origin`, `unauthorized_path`, `unreadable_file`, `unstable_file_timeout`, `artifact_not_found`, `review_not_found`, and `ui_delivery_failed`. Emit structured stderr using the existing `CliError`; do not fall back to disk for `present`. `show` and `review show` may use read-only disk fallback only after validating the artifact store schema.

- [ ] **Step 6: Run CLI tests**

Run: `cargo test -p wardian-cli artifact -- --test-threads=1`

Run: `cargo test -p wardian-cli -- --test-threads=1`

Expected: parser, request serialization, exit code, error, and output tests pass.

- [ ] **Step 7: Commit the CLI protocol**

```bash
git add crates/wardian-core/src/control.rs crates/wardian-cli
git commit -m "feat(cli): add artifact presentation commands"
```

### Task 4: Dispatch artifact control requests and acknowledge background Workbench routing

**Files:**
- Modify: `src-tauri/src/control.rs`
- Create: `src-tauri/src/commands/artifacts.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/features/workbench/workbenchModel.ts`
- Modify: `src/features/workbench/workbenchModel.test.ts`
- Modify: `src/features/workbench/navigationService.ts`
- Modify: `src/features/workbench/navigationService.test.ts`
- Create: `src/features/files/useArtifactEvents.ts`
- Create: `src/features/files/useArtifactEvents.test.tsx`
- Modify: `src/views/App.tsx`

**Interfaces:**
- Consumes: Task 2 service/runtime, Task 3 `ControlRequest` variants, Workbench model/store, and Tauri event/invoke APIs.
- Produces: control dispatch, `ack_artifact_presentation`, `open_background`, `ArtifactPresentedEventV1`, and event subscription mounted once by `App`.

- [ ] **Step 1: Add failing background-open and acknowledgement tests**

Assert `open_surface` defaults to active, `activate: false` preserves the active surface/group, existing artifact reuse does not focus it, attention is set for new and reused tabs, acknowledgement occurs only after the Workbench transaction succeeds, and navigation failure returns a failed ack.

```ts
const previous = store.getState().document.groups["group-1"].active_surface_id;
navigation.open_background(artifactRequest);
expect(store.getState().document.groups["group-1"].active_surface_id).toBe(previous);
```

- [ ] **Step 2: Run focused tests and confirm open always activates**

Run: `npm run test -- --run src/features/workbench/workbenchModel.test.ts src/features/workbench/navigationService.test.ts src/features/files/useArtifactEvents.test.tsx`

Expected: `open_surface` changes active surface and the artifact event hook is missing.

- [ ] **Step 3: Add a model-level non-activating open**

Extend the command to:

```ts
{ type: "open_surface"; surface: WorkbenchSurfaceV1; group_id?: string; index?: number; activate?: boolean }
```

Default `activate` to true for every existing caller. With false, append the tab but preserve the group's prior `active_surface_id` and `document.active_group_id`. Validate the target group and surface exactly as the active path does.

- [ ] **Step 4: Add `open_background` to NavigationService**

Create or resolve the artifact resource in the active group without focus. On reuse, update state to the latest `selected_version_id`, set attention in `filesPresentationStore`, and leave the current surface active. Artifact state is permanent: `transient_preview: false`.

- [ ] **Step 5: Dispatch control and acknowledge from React**

`src-tauri/src/control.rs` validates `MessageOrigin`, calls `ArtifactService`, and serializes the typed response. `ack_artifact_presentation` resolves the pending oneshot only once. `useArtifactEvents` listens once at App scope, calls `open_background`, then invokes the ack with `presentation_id`, `artifact_id`, and `surface_id`; rejected Workbench transactions ack with `ok: false` and an error code.

- [ ] **Step 6: Run frontend and Rust tests**

Run: `npm run test -- --run src/features/workbench/workbenchModel.test.ts src/features/workbench/navigationService.test.ts src/features/files/useArtifactEvents.test.tsx`

Run: `cargo test -p Wardian artifact -- --test-threads=1`

Expected: active-tab preservation, event ordering, one-shot ack, and control serialization pass.

- [ ] **Step 7: Commit background delivery**

```bash
git add src-tauri/src/control.rs src-tauri/src/commands src-tauri/src/lib.rs src/features/workbench src/features/files/useArtifactEvents.ts src/features/files/useArtifactEvents.test.tsx src/views/App.tsx
git commit -m "feat(artifacts): route presentations in background"
```

### Task 5: Artifact versions, provenance, live working state, and attention UI

**Files:**
- Modify: `src/types/files.ts`
- Modify: `src/features/files/fileResourceClient.ts`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesModeBar.tsx`
- Create: `src/features/files/ArtifactDetails.tsx`
- Create: `src/features/files/ArtifactDetails.test.tsx`
- Create: `src/features/files/useArtifactResource.ts`
- Create: `src/features/files/useArtifactResource.test.tsx`
- Modify: `src/features/files/filesPresentationStore.ts`
- Modify: `src/features/files/FilesSurface.test.tsx`
- Modify: `src-tauri/src/commands/artifacts.rs`

**Interfaces:**
- Consumes: artifact manifest/version DTOs, Plan 1 live file revisions, `artifact:<id>`, and presentation attention.
- Produces: `get_artifact_resource`, `mark_artifact_attention_read`, version selector, provenance summary, `Changed since presented`, and attention badges.

- [ ] **Step 1: Add failing artifact-controller tests**

Assert current working descriptor and selected immutable version load separately; watcher refresh updates working hash without appending a version; hash mismatch shows `Changed since presented`; selecting a version changes preview but not working state; attention clears only after the artifact tab becomes visible; and missing manifests remain resource-local.

- [ ] **Step 2: Run focused tests and confirm artifact resources are not rendered**

Run: `npm run test -- --run src/features/files/useArtifactResource.test.tsx src/features/files/ArtifactDetails.test.tsx src/features/files/FilesSurface.test.tsx`

Expected: artifact controller and details imports fail.

- [ ] **Step 3: Add one aggregate artifact-resource command**

`get_artifact_resource(artifact_id, selected_version_id)` returns the manifest summary, selected version descriptor/read ticket source, current working descriptor, comparison hashes, attention state, and typed errors. It never returns every version's bytes. `mark_artifact_attention_read` updates the index/manifest atomically.

- [ ] **Step 4: Implement artifact rendering in the existing Files shell**

Reuse the same renderer registry. Preview defaults to the current working state while the version selector can inspect immutable presented versions. Show concise origin, presented time, version number, and status in `ArtifactDetails`. Show `Changed since presented` when current and selected hashes differ. Do not add a nested tab bar.

- [ ] **Step 5: Clear attention on actual visibility**

Use the Workbench lifecycle `visible` state, not mount, hover, or event receipt. Update the presentation store and backend only after the artifact surface becomes visible. A background tab retains its badge.

- [ ] **Step 6: Run focused tests**

Run: `npm run test -- --run src/features/files/useArtifactResource.test.tsx src/features/files/ArtifactDetails.test.tsx src/features/files/FilesSurface.test.tsx`

Expected: versions, provenance, working-state drift, attention, and error tests pass.

- [ ] **Step 7: Commit artifact presentation UI**

```bash
git add src/types/files.ts src/features/files src-tauri/src/commands/artifacts.rs
git commit -m "feat(files): show artifact versions and provenance"
```

### Task 6: Queue attention and Quick Open/recent discovery

**Files:**
- Modify: `src/types/index.ts`
- Modify: `src/store/useQueueStore.ts`
- Modify: `src/store/useQueueStore.test.ts`
- Modify: `src/views/QueueView.tsx`
- Modify: `src/views/QueueView.test.tsx`
- Create: `src/features/workbench/quickOpenResources.ts`
- Create: `src/features/workbench/quickOpenResources.test.ts`
- Modify: `src/features/workbench/OpenSurfaceDialog.tsx`
- Modify: `src/features/workbench/OpenSurfaceDialog.test.tsx`
- Modify: `src/features/workbench/HomeSurface.tsx`
- Modify: `src/features/workbench/HomeSurface.test.tsx`
- Modify: `src/views/App.tsx`

**Interfaces:**
- Consumes: `ArtifactPresentedEventV1`, artifact recent summaries, Queue persistence, OpenSurfaceDialog query, and Workbench navigation.
- Produces: `artifact_review_needed` Queue items, artifact deep links, and searchable/recent artifact results in Quick Open and empty-pane home.

- [ ] **Step 1: Add failing discovery tests**

Assert presentation adds one unread Queue item per artifact/version, reuse updates rather than duplicates stale attention, Queue click opens/focuses the artifact, closing the tab leaves Queue/recent intact, Quick Open matches title/path/origin, and resolving attention does not delete history.

- [ ] **Step 2: Run focused tests**

Run: `npm run test -- --run src/store/useQueueStore.test.ts src/views/QueueView.test.tsx src/features/workbench/quickOpenResources.test.ts src/features/workbench/OpenSurfaceDialog.test.tsx src/features/workbench/HomeSurface.test.tsx`

Expected: `artifact_review_needed` and resource results are unsupported.

- [ ] **Step 3: Extend Queue DTOs and preferences**

Add `artifact_review_needed` to `QueueEventType` and `QueueItem.type`. Artifact items carry `artifact_id`, `version_id`, title, origin agent name, canonical path, and summary but no file bytes. Update preference normalization so old persisted maps receive a default visible/notification value without reset.

- [ ] **Step 4: Add one Quick Open resource provider**

`quickOpenResources.ts` merges open Files resources, backend `list_recent_artifacts(limit: 20)`, and recent file resources, deduplicated by resource key. `OpenSurfaceDialog` searches these alongside surface types and opens through NavigationService. Home shows at most the most recent actionable artifact in its existing recent area, not a new Artifacts collection.

- [ ] **Step 5: Route Queue and recent entries**

Queue click marks the item read and opens/focuses `artifact:<id>`. Approval/review states later resolve actionability but do not erase the item. A missing thread shows a typed unavailable Files tab so history remains inspectable.

- [ ] **Step 6: Run focused discovery tests**

Run: `npm run test -- --run src/store/useQueueStore.test.ts src/views/QueueView.test.tsx src/features/workbench/quickOpenResources.test.ts src/features/workbench/OpenSurfaceDialog.test.tsx src/features/workbench/HomeSurface.test.tsx`

Expected: Queue, Quick Open, close/reopen, and recent tests pass.

- [ ] **Step 7: Commit artifact discovery**

```bash
git add src/types/index.ts src/store/useQueueStore.ts src/store/useQueueStore.test.ts src/views/QueueView.tsx src/views/QueueView.test.tsx src/features/workbench src/views/App.tsx
git commit -m "feat(artifacts): add queue and quick open discovery"
```

### Task 7: Native CLI/restart proof and lifecycle documentation

**Files:**
- Create: `e2e-native/tests/artifact-lifecycle-native.test.mjs`
- Modify: `e2e-native/lib/harness.mjs`
- Modify: `e2e/fixtures/workbenchIpcMock.ts`
- Create: `e2e/tests/artifact-lifecycle.spec.ts`
- Modify: `docs/guide/cli.md`
- Modify: `docs/guide/workbench.md`
- Modify: `docs/guide/queue.md`
- Create: `docs/guide/artifacts.md`
- Modify: `docs/developer/tauri-command-reference.md`
- Modify: `docs/developer/ipc-events.md`
- Modify: `docs/developer/state-management.md`

**Interfaces:**
- Consumes: a real staged CLI, shared native `WARDIAN_HOME`, a seeded mock agent/session, Workbench persistence, and the artifact store.
- Produces: end-to-end evidence for authorization, reuse, background attention, retrieval, restart, and failure codes; user/developer lifecycle documentation.

- [ ] **Step 1: Add failing native lifecycle scenarios**

Run the real CLI inside a seeded session to present an authorized file. Assert structured fields, inactive tab focus preservation, attention, Queue item, same-thread reuse, `--new`, explicit target, unauthorized rejection, app-not-running rejection, `show`, and persistence after app restart.

- [ ] **Step 2: Run the targeted native test before final wiring**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-lifecycle-native.test.mjs`

Expected before completion: `wardian artifact` or the control request fails.

- [ ] **Step 3: Add browser lifecycle coverage**

Mock a presentation event and acknowledgement; verify the active tab stays active, an artifact tab/badge appears, Queue opens it, versions render, close leaves Quick Open discovery, and an event failure sends a failed ack. Browser coverage does not claim filesystem or control transport behavior.

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/artifact-lifecycle.spec.ts`

Expected: all mocked UI lifecycle assertions pass.

- [ ] **Step 4: Document CLI and lifecycle**

Document exact present/show/review-show syntax, structured response/error fields, same-home/session requirements, thread reuse, immutable versions, background attention, working-state drift, Queue/recent discovery, and the fact that review submission arrives in Plan 3. Use cross-platform placeholders and POSIX shell before PowerShell.

- [ ] **Step 5: Run the slice verification suite**

Run: `npm run lint`

Run: `npm run test`

Run: `npm run build`

Run: `npm run test:e2e -- e2e/tests/artifact-lifecycle.spec.ts`

Run: `cargo check --workspace`

Run: `cargo clippy --workspace -- -D warnings`

Run: `cargo test --workspace -- --test-threads=1`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-lifecycle-native.test.mjs`

Expected: every command passes and the Files launcher remains reserved.

- [ ] **Step 6: Capture PR evidence and commit tests/docs**

Capture the background artifact attention and Queue-open interaction under `e2e/screenshots/files-surface/<timestamp>/`, upload it, and embed its HTTPS URL in the PR description.

```bash
git add e2e-native e2e docs/guide docs/developer
git commit -m "test(artifacts): prove durable presentation lifecycle"
```
