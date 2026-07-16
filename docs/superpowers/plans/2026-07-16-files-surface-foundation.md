# Files Surface Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the unbounded Explorer preview path with a backend-owned, resource-keyed Files foundation that safely renders ordinary text, Markdown, images, and PDFs in Workbench tabs.

**Architecture:** `wardian-core` owns canonical paths, authorization, content descriptors, hashes, and limit decisions. Tauri owns exact-file capabilities, reference-counted watches, stable revisions, and scoped read tickets. React receives descriptors and revision events, selects a lazy renderer, and uses Workbench resource identity for transient or permanent presentations.

**Tech Stack:** Rust, Tauri 2, React 19, TypeScript, Zustand, Dockview, Monaco Editor, PDF.js, React Markdown, Vitest, Playwright, and native E2E.

## Global Constraints

- Implement against `docs/specs/2026-07-16-files-artifact-review-surface.md` and the cross-plan ledger in `docs/superpowers/plans/2026-07-16-files-artifact-review-surface.md`.
- Keep the New Surface contribution labeled **Files** but `reserved: true`; activation belongs only to Plan 4.
- File resource keys are `file:<canonical-normalized-path>`. Do not key ordinary files by title, inode, raw user input, or tab ID.
- Agent authorization uses `folder` plus `include_directories`; exclude `system_include_directories` in code and tests.
- A native picker grants one exact canonical file, not its parent directory.
- File content never enters the Workbench document or Zustand persistence.
- All reads are revision-bound and limit-checked. A read ticket cannot be replayed for another path or revision.
- Watchers emit one stable revision after the file stops changing; raw notify bursts do not reach React.
- Use the existing Workbench tab strip. Files adds no nested tabs.
- Draft, Changes, artifact presentation, live HTML/SVG, and review actions remain unavailable until their owning plans land.

---

### Task 1: Canonical authorized roots and file descriptors

**Files:**
- Modify: `crates/wardian-core/src/lib.rs`
- Create: `crates/wardian-core/src/files/mod.rs`
- Create: `crates/wardian-core/src/files/authorized_roots.rs`
- Create: `crates/wardian-core/src/files/descriptor.rs`
- Test: `crates/wardian-core/src/files/authorized_roots.rs`
- Test: `crates/wardian-core/src/files/descriptor.rs`

**Interfaces:**
- Consumes: `wardian_core::models::AgentConfig.folder`, `include_directories`, and `system_include_directories`.
- Produces: `AuthorizedRootService`, `AuthorizedPath`, `FileContentDescriptorV1`, `FileRendererKind`, `FileResourceCapabilitiesV1`, `FileResourceLimits`, and `FileResourceErrorV1`.

- [ ] **Step 1: Add failing authorization and detection tests**

Cover a primary workspace, one additional directory, an excluded system include, `..` traversal, a symlink or Windows junction that escapes a root, a missing file, UTF-8 text, Markdown, PNG signature, PDF signature, binary data, and every size/line limit boundary.

```rust
#[test]
fn agent_roots_exclude_system_include_directories() {
    let config = config_with_roots("workspace", &["shared"], &["managed-skills"]);
    let service = AuthorizedRootService::from_agent_config(&config).expect("valid roots");
    assert!(service.authorize_existing_file(Path::new("workspace/report.md")).is_ok());
    assert!(service.authorize_existing_file(Path::new("shared/figure.png")).is_ok());
    assert_eq!(
        service.authorize_existing_file(Path::new("managed-skills/secret.md"))
            .expect_err("system include must be rejected")
            .code(),
        "unauthorized_path",
    );
}
```

- [ ] **Step 2: Run the focused core tests and confirm the module is missing**

Run: `cargo test -p wardian-core files:: -- --test-threads=1`

Expected: compilation fails because `wardian_core::files` and its types do not exist.

- [ ] **Step 3: Implement fail-closed canonical authorization**

Expose these signatures from `authorized_roots.rs`:

```rust
pub struct AuthorizedRootService {
    roots: Vec<PathBuf>,
}

impl AuthorizedRootService {
    pub fn from_agent_config(config: &AgentConfig) -> Result<Self, FileResourceErrorV1>;
    pub fn roots(&self) -> &[PathBuf];
    pub fn authorize_existing_file(&self, requested: &Path) -> Result<AuthorizedPath, FileResourceErrorV1>;
}

pub struct AuthorizedPath {
    pub canonical_path: PathBuf,
    pub root: PathBuf,
}
```

Canonicalize every configured root and the requested file before containment. Compare path components, not string prefixes. On Windows, normalize drive-letter casing after `canonicalize`; do not lowercase arbitrary path segments. Reject a root or target that cannot be resolved.

- [ ] **Step 4: Implement descriptor detection and central limits**

Define versioned snake_case DTOs with these stable fields:

```rust
pub struct FileContentDescriptorV1 {
    pub schema: u8,
    pub canonical_path: String,
    pub display_name: String,
    pub extension: Option<String>,
    pub mime_type: String,
    pub encoding: Option<String>,
    pub renderer_kind: FileRendererKind,
    pub size_bytes: u64,
    pub line_count: Option<u64>,
    pub content_hash: String,
    pub modified_at_ms: u64,
    pub capabilities: FileResourceCapabilitiesV1,
    pub unavailable_reason: Option<String>,
}
```

Use extension only as a hint. Confirm PNG/JPEG/GIF/WebP and PDF from signatures, classify UTF-8 text after validation, and return `Unsupported` for invalid text/binary data. Central defaults are 16 MiB/200,000 lines for Monaco, 5 MiB per diff side/100,000 lines for later diff, 64 MiB and 64 MP for images, and 256 MiB for PDFs.

- [ ] **Step 5: Run focused and crate tests**

Run: `cargo test -p wardian-core files:: -- --test-threads=1`

Run: `cargo test -p wardian-core -- --test-threads=1`

Expected: authorization, escape rejection, content detection, and limit tests pass.

- [ ] **Step 6: Commit the core foundation**

```bash
git add crates/wardian-core/src/lib.rs crates/wardian-core/src/files
git commit -m "feat(files): add authorized file descriptors"
```

### Task 2: Tauri file-resource runtime, stable watches, and read tickets

**Files:**
- Modify: `src-tauri/src/state/mod.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Create: `src-tauri/src/state/file_resources.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Create: `src-tauri/src/commands/files.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/state/file_resources.rs`
- Test: `src-tauri/src/commands/files.rs`

**Interfaces:**
- Consumes: `AuthorizedRootService`, `FileContentDescriptorV1`, Tauri `AppHandle`, agent configuration from `AppState`, and `notify::RecommendedWatcher`.
- Produces Tauri commands `open_file_resource`, `close_file_resource`, `read_file_resource_text`, `issue_file_resource_ticket`, and `pick_file_resource`; emits `file-resource://revision` with `FileResourceEventV1`.

- [ ] **Step 1: Add failing runtime tests**

Test two subscribers to one canonical file share one watcher, close decrements the reference count, a three-write burst produces one revision after 150 ms stability, a revision changes only when the content hash changes, stale text reads fail, ticket scope is exact, native-picker grants do not authorize siblings, and a revoked agent root invalidates the next read.

```rust
#[tokio::test]
async fn coalesces_write_bursts_into_one_stable_revision() {
    let runtime = test_runtime(Duration::from_millis(150));
    let subscription = runtime.open_agent_file(test_agent(), test_file()).await.unwrap();
    write_three_times(test_file()).await;
    let event = runtime.next_event(&subscription.subscription_id).await.unwrap();
    assert_eq!(event.revision, 2);
    assert!(runtime.try_next_event(&subscription.subscription_id).is_none());
}
```

- [ ] **Step 2: Run the focused Tauri tests and confirm missing runtime failures**

Run: `cargo test -p Wardian file_resources -- --test-threads=1`

Expected: compilation fails because `FileResourceRuntime` and command DTOs do not exist.

- [ ] **Step 3: Implement one backend-owned runtime**

Add this state boundary:

```rust
pub struct FileResourceRuntime {
    entries: tokio::sync::Mutex<HashMap<String, FileResourceEntry>>,
    user_file_grants: tokio::sync::Mutex<HashMap<String, UserFileGrant>>,
    read_tickets: tokio::sync::Mutex<HashMap<String, FileReadTicket>>,
    limits: FileResourceLimits,
}

pub struct FileResourceSnapshotV1 {
    pub resource_id: String,
    pub subscription_id: String,
    pub revision: u64,
    pub descriptor: FileContentDescriptorV1,
}
```

Key entries by canonical path. Keep subscriber IDs separate from resources. Hash only after the stable-write timer fires; emit no event when bytes/hash are unchanged. Re-check the authorization capability on every open, read, and ticket issue.

- [ ] **Step 4: Add typed Tauri commands and resource-local errors**

Use request DTOs rather than positional strings:

```rust
pub struct OpenFileResourceRequestV1 {
    pub path: String,
    pub agent_id: Option<String>,
    pub user_file_capability_id: Option<String>,
}

#[tauri::command]
pub async fn open_file_resource(
    request: OpenFileResourceRequestV1,
    state: tauri::State<'_, AppState>,
) -> Result<FileResourceSnapshotV1, FileResourceErrorV1>;
```

`pick_file_resource` invokes the native file picker and records an exact-file grant. `read_file_resource_text` requires `(resource_id, revision)` and returns only validated, limit-compliant UTF-8. `issue_file_resource_ticket` returns an unguessable renderer-lease-scoped URL with a 60-second expiry for image/PDF streaming. Register a `wardian-resource` protocol handler in `lib.rs`; it serves repeated byte ranges only for the descriptor's exact revision while the lease remains active, with `nosniff`, a concrete MIME type, and no-store caching.

- [ ] **Step 5: Register commands, state, protocol, and event cleanup**

Add `FileResourceRuntime::default()` to `AppState`, export the module, register all commands in `tauri::generate_handler!`, and close remaining subscriptions during application exit. Do not add the resource protocol to live HTML/SVG; Plan 4 owns executable local dependencies.

- [ ] **Step 6: Run Tauri and workspace checks**

Run: `cargo test -p Wardian file_resources -- --test-threads=1`

Run: `cargo check --workspace`

Expected: runtime, command, stable-write, exact grant, range, stale ticket, and authorization tests pass.

- [ ] **Step 7: Commit the native resource service**

```bash
git add src-tauri/src/state src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(files): add native file resource runtime"
```

### Task 3: Frontend DTOs, clients, and renderer dependencies

**Files:**
- Modify: `package.json`
- Modify: `package-lock.json`
- Modify: `vite.config.ts`
- Modify: `src/types/index.ts`
- Create: `src/types/files.ts`
- Create: `src/features/files/fileResourceKey.ts`
- Create: `src/features/files/fileResourceClient.ts`
- Create: `src/features/files/useFileResource.ts`
- Test: `src/features/files/fileResourceKey.test.ts`
- Test: `src/features/files/fileResourceClient.test.ts`
- Test: `src/features/files/useFileResource.test.tsx`

**Interfaces:**
- Consumes: Plan 1 Tauri command/event DTOs and `@tauri-apps/api` invoke/listen APIs.
- Produces: `FileResourceKey`, `FilesSurfaceStateV1`, `FileContentDescriptorV1`, `FileResourceSnapshotV1`, `FileResourceClient`, and `useFileResource`.

- [ ] **Step 1: Add failing identity and lifecycle tests**

Assert slash normalization without case-folding arbitrary path segments, distinct `file:` versus `artifact:` identities, exact snake_case invoke payloads, one listener per mounted controller, revision reload after a matching event, ignored unrelated events, and subscription cleanup on unmount.

```ts
expect(fileResourceKey('C:\\work\\notes.md')).toBe('file:C:/work/notes.md');
expect(fileResourceKey('/work/report.md')).not.toBe(artifactResourceKey('/work/report.md'));
```

- [ ] **Step 2: Run focused tests and confirm missing modules**

Run: `npm run test -- --run src/features/files/fileResourceKey.test.ts src/features/files/fileResourceClient.test.ts src/features/files/useFileResource.test.tsx`

Expected: the tests fail because the Files client modules do not exist.

- [ ] **Step 3: Install and pin renderer dependencies**

Run: `npm install --save-exact @monaco-editor/react monaco-editor pdfjs-dist`

Expected: `package.json` and `package-lock.json` record exact resolved versions. Do not use CDN workers or runtime network imports.

Configure Vite worker loading through local ESM imports. The production build must contain Monaco and PDF.js worker assets with hashed local filenames.

- [ ] **Step 4: Define the shared frontend contract**

Put Files DTOs in `src/types/files.ts` and export them from `src/types/index.ts`. Define the approved bounded state exactly:

```ts
export type FilesSurfaceStateV1 = {
  resource_kind: "file" | "artifact";
  mode: "preview" | "changes" | "draft";
  transient_preview: boolean;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
};
```

Mirror Rust enum strings and property names. Do not add file bytes, draft text, comments, or capabilities to surface state.

- [ ] **Step 5: Implement the invoke adapter and hook**

`FileResourceClient.open(request)` returns a snapshot; `readText(resource_id, revision)` and `issueTicket(resource_id, revision, purpose)` are revision-bound; `close(subscription_id)` is idempotent. `useFileResource` exposes `{ status, snapshot, error, retry }`, never throws into the application error boundary, and closes the subscription only after the last consumer releases it.

- [ ] **Step 6: Run focused tests, lint, and build**

Run: `npm run test -- --run src/features/files/fileResourceKey.test.ts src/features/files/fileResourceClient.test.ts src/features/files/useFileResource.test.tsx`

Run: `npm run lint`

Run: `npm run build`

Expected: all focused tests pass and production worker assets are emitted locally.

- [ ] **Step 7: Commit the frontend resource contract**

```bash
git add package.json package-lock.json vite.config.ts src/types src/features/files
git commit -m "feat(files): add frontend resource client"
```

### Task 4: Register the internal Files surface and presentation metadata

**Files:**
- Modify: `src/features/workbench/coreSurfaceRegistry.ts`
- Modify: `src/features/workbench/coreSurfaceRegistry.test.ts`
- Modify: `src/features/workbench/navigationService.ts`
- Modify: `src/features/workbench/navigationService.test.ts`
- Create: `src/features/files/filesPresentationStore.ts`
- Test: `src/features/files/filesPresentationStore.test.ts`

**Interfaces:**
- Consumes: `FilesSurfaceStateV1`, `fileResourceKey`, `WorkbenchSurfaceRegistry`, and `WorkbenchNavigationService`.
- Produces: internal `files` surface definition, dynamic basename/type-icon metadata, dirty/attention badges, `open_transient`, and `pin_transient` navigation methods.

- [ ] **Step 1: Add failing registry and navigation tests**

Assert `files` uses `suspend_when_hidden`, `focus_resource`, `view_only`, and `confirm_if_dirty`; invalid state fails restoration; file/artifact keys do not collide; a second transient file replaces the existing transient in the same group; a transient in another group is untouched; double-click/pin updates `transient_preview` to false; and a permanent file is focused rather than replaced.

```ts
expect(registry.require("files")).toMatchObject({
  render_policy: "suspend_when_hidden",
  open_policy: "focus_resource",
  close_policy: "confirm_if_dirty",
});
```

- [ ] **Step 2: Run focused tests and confirm Files is unregistered**

Run: `npm run test -- --run src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/navigationService.test.ts src/features/files/filesPresentationStore.test.ts`

Expected: registry lookup and transient navigation tests fail.

- [ ] **Step 3: Add a strict Files state restorer**

Register `files` in `coreSurfaceDefinitions()` while leaving `CORE_SURFACE_CONTRIBUTIONS` reserved:

```ts
{ surface_type: "files", title: "Files", description: "Inspect files and agent artifacts.", group: "Reserved", reserved: true }
```

Remove the old `file-editor` contribution. The definition requires a `file:` or `artifact:` resource key. Its title and icon consult `filesPresentationStore`, falling back to a basename and descriptor-family icon. Badges expose only `dirty` and `attention` with accessible labels.

- [ ] **Step 4: Implement group-local transient replacement**

Extend `WorkbenchNavigationService`:

```ts
open_transient(request: OpenSurfaceRequest): string;
pin_transient(surface_id: string): void;
```

`open_transient` resolves a matching permanent resource first. Otherwise, it replaces the target group's current Files surface whose restored state has `transient_preview: true`, preserving that surface ID and issuing one `replace_surface` transaction. `pin_transient` issues `update_surface_state` with `transient_preview: false`. Never scan or replace another group.

- [ ] **Step 5: Wire the close guard boundary**

`filesPresentationStore` records descriptor metadata, attention, and dirty state by `surface_id`. For now dirty is always false; Plan 3 sets it from durable drafts. The close guard calls the existing injected dirty prompt only when dirty is true and otherwise allows immediately.

- [ ] **Step 6: Run focused Workbench tests**

Run: `npm run test -- --run src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/navigationService.test.ts src/features/files/filesPresentationStore.test.ts`

Expected: Files restoration, identity, metadata fallback, badges, transient replacement, and pinning pass.

- [ ] **Step 7: Commit the internal surface contract**

```bash
git add src/features/workbench src/features/files/filesPresentationStore.ts src/features/files/filesPresentationStore.test.ts
git commit -m "feat(workbench): register files resource surfaces"
```

### Task 5: Files shell and renderer registry

**Files:**
- Create: `src/features/files/rendererRegistry.ts`
- Create: `src/features/files/rendererRegistry.test.ts`
- Create: `src/features/files/FilesSurface.tsx`
- Create: `src/features/files/FilesSurface.test.tsx`
- Create: `src/features/files/FilesModeBar.tsx`
- Create: `src/features/files/FilePreview.tsx`
- Create: `src/features/files/UnsupportedRenderer.tsx`
- Create: `src/features/files/FilesSurface.css`
- Modify: `src/views/App.tsx`
- Test: `src/views/App.test.tsx`

**Interfaces:**
- Consumes: `WorkbenchSurfaceV1`, surface lifecycle, `useFileResource`, and `FileContentDescriptorV1`.
- Produces: `FileRendererDefinition`, `RendererRegistry`, Files pane shell, resource-local error states, and the `files` branch in `renderWorkbenchSurface`.

- [ ] **Step 1: Add failing renderer-selection and shell tests**

Test selection by descriptor `renderer_kind` rather than extension, Preview as the only enabled mode, breadcrumb/metadata overflow, Retry/Open With/Reveal states, one compact header row, narrow-pane overflow behavior, and a thrown renderer contained within the Files pane.

```ts
expect(registry.resolve(pdfDescriptorWithTxtExtension()).renderer_id).toBe("pdf");
expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute("aria-selected", "true");
expect(screen.getByRole("tab", { name: "Changes" })).toHaveAttribute("aria-disabled", "true");
```

- [ ] **Step 2: Run focused tests and confirm the shell is missing**

Run: `npm run test -- --run src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.test.tsx src/views/App.test.tsx`

Expected: module imports and App surface rendering fail.

- [ ] **Step 3: Implement an extension-ready registry**

Define:

```ts
export type FileRendererDefinition = {
  renderer_id: string;
  matches: (descriptor: FileContentDescriptorV1) => boolean;
  capabilities: {
    preview: boolean;
    changes: "line" | "version" | "none";
    draft: boolean;
    annotations: "line_range" | "spatial" | "general";
  };
  render: React.LazyExoticComponent<React.ComponentType<FileRendererProps>>;
};
```

Resolution order is explicit renderer kind, validated MIME fallback, then unsupported. Reject duplicate `renderer_id` values at registry construction.

- [ ] **Step 4: Build the responsive content-first shell**

Use one `.files-mode-bar`, one remaining-space content region, and no nested tab strip. Disable unavailable modes with an explanation. Put secondary metadata actions in one overflow menu. Catch renderer errors with a resource-local boundary offering Reset Renderer and Open With; do not throw to the fatal application boundary.

- [ ] **Step 5: Render Files from App without activating its launcher card**

Add a `files` branch to `renderWorkbenchSurface` that passes `surface_id`, `resource_key`, restored state, lifecycle, and the file client. Keep the New Surface contribution reserved and unselectable.

- [ ] **Step 6: Run focused tests**

Run: `npm run test -- --run src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.test.tsx src/views/App.test.tsx`

Expected: shell, error containment, and mode availability tests pass.

- [ ] **Step 7: Commit the Files shell**

```bash
git add src/features/files src/views/App.tsx src/views/App.test.tsx
git commit -m "feat(files): add workbench files shell"
```

### Task 6: Monaco text, Markdown, image, PDF, and unsupported renderers

**Files:**
- Create: `src/features/files/renderers/MonacoTextRenderer.tsx`
- Create: `src/features/files/renderers/MonacoTextRenderer.test.tsx`
- Create: `src/features/files/renderers/MarkdownRenderer.tsx`
- Create: `src/features/files/renderers/MarkdownRenderer.test.tsx`
- Create: `src/features/files/renderers/ImageRenderer.tsx`
- Create: `src/features/files/renderers/ImageRenderer.test.tsx`
- Create: `src/features/files/renderers/PdfRenderer.tsx`
- Create: `src/features/files/renderers/PdfRenderer.test.tsx`
- Create: `src/features/files/renderers/pdfWorker.ts`
- Modify: `src/features/files/rendererRegistry.ts`

**Interfaces:**
- Consumes: revision-bound text reads, renderer-lease-scoped resource tickets, renderer props, descriptor capabilities, Monaco, PDF.js, and existing Wardian Markdown safety utilities.
- Produces: read-only text/code preview, rendered Markdown, image fit/actual/zoom/pan, PDF pages/search/zoom, and metadata-only fallback.

- [ ] **Step 1: Add failing renderer behavior tests**

Assert Monaco uses `readOnly: true`, automatic layout is driven by pane ResizeObserver, models are keyed by `resource_id@revision`, Markdown strips active HTML, image object URLs/tickets are released on revision/unmount, PDF.js uses the bundled worker and range ticket, and oversized/unsupported descriptors never invoke content reads.

- [ ] **Step 2: Run renderer tests and confirm missing implementations**

Run: `npm run test -- --run src/features/files/renderers`

Expected: imports fail for the four renderer modules.

- [ ] **Step 3: Implement Monaco and Markdown preview**

Lazy-load Monaco. Detect language from descriptor MIME/extension and use `plaintext` when unknown. Dispose models only when the shared resource reference count reaches zero. Render Markdown with `react-markdown` and `remark-gfm`; reuse `markdownSafety.ts` URL rules, disable raw HTML, and route local linked files through trusted Wardian navigation rather than the browser.

- [ ] **Step 4: Implement image and PDF preview**

Images support Fit, 100%, zoom, and pan while enforcing descriptor limits before ticket issue. PDF.js receives a local `wardian-resource` URL, displays incremental pages, search, and zoom, and cancels render tasks on suspension or revision changes. Neither renderer copies whole binary payloads into React state.

- [ ] **Step 5: Register renderer capabilities**

Text/Markdown report line changes and Draft capability for later plans. Images/PDF report version changes, no Draft, and spatial annotations for later plans. HTML/active SVG remain `UnsupportedRenderer` with the reason `live_renderer_not_activated` until Plan 4.

- [ ] **Step 6: Run renderer tests and production build**

Run: `npm run test -- --run src/features/files/renderers src/features/files/rendererRegistry.test.ts`

Run: `npm run build`

Expected: renderer tests pass, all workers are local build assets, and no CDN URL appears in `dist`.

- [ ] **Step 7: Commit safe first-release renderers**

```bash
git add src/features/files/renderers src/features/files/rendererRegistry.ts
git commit -m "feat(files): render text markdown images and pdfs"
```

### Task 7: Route Explorer into transient and permanent Files tabs

**Files:**
- Modify: `src/features/explorer/FileTree.tsx`
- Modify: `src/features/explorer/FileTree.test.tsx`
- Modify: `src/features/explorer/ExplorerPanel.tsx`
- Modify: `src/features/explorer/ExplorerPanel.test.tsx`
- Modify: `src/layout/AppShell.tsx`
- Test: `e2e/fixtures/workbenchIpcMock.ts`
- Create: `e2e/tests/files-surface-foundation.spec.ts`

**Interfaces:**
- Consumes: Workbench `open_transient`, normal `open`, `open_to_side`, existing Explorer click-action setting, and exact file paths from `FileTree`.
- Produces: single-click transient preview, double-click permanent open, Open/Open to Side context actions, and removal of the `read_file_preview` modal path.

- [ ] **Step 1: Add failing Explorer interaction tests**

Assert single click calls `open_transient`, double click pins or opens permanent without a second transient tab, context Open is permanent, Open to Side uses the current group, external-editor preference still opens externally, and no `read_file_preview` invoke occurs.

- [ ] **Step 2: Run focused Explorer tests and confirm modal behavior remains**

Run: `npm run test -- --run src/features/explorer/FileTree.test.tsx src/features/explorer/ExplorerPanel.test.tsx`

Expected: internal clicks still invoke `read_file_preview` and no double-click contract exists.

- [ ] **Step 3: Add explicit select/open events to FileTree**

Keep `onSelect(path, is_dir)` for single-click selection and add `onOpen(path, is_dir)` for double-click/Enter. Suppress the delayed single-click replacement when a double-click opens the same item permanently. Directory behavior remains expand/collapse.

- [ ] **Step 4: Replace the Explorer modal with NavigationService calls**

Delete preview title/content modal state and `openPreview`. Internal single click constructs a `files` request with `transient_preview: true`; double click and context Open use `transient_preview: false`; Open to Side uses the standard horizontal split admission. Preserve external-editor behavior when configured.

- [ ] **Step 5: Add browser E2E with mocked native descriptors**

Seed two text files, single-click A then B and assert one transient tab changes identity, double-click B and assert it becomes permanent, single-click A and assert a second transient appears, and verify Open to Side creates a normal Workbench group. Mock only descriptors/text; do not claim filesystem or protocol coverage.

- [ ] **Step 6: Run unit and browser tests**

Run: `npm run test -- --run src/features/explorer/FileTree.test.tsx src/features/explorer/ExplorerPanel.test.tsx`

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/files-surface-foundation.spec.ts`

Expected: transient replacement, pinning, permanent open, and split behavior pass.

- [ ] **Step 7: Commit Explorer routing**

```bash
git add src/features/explorer src/layout/AppShell.tsx e2e/fixtures/workbenchIpcMock.ts e2e/tests/files-surface-foundation.spec.ts
git commit -m "feat(explorer): open files in workbench tabs"
```

### Task 8: Native foundation proof and documentation

**Files:**
- Modify: `e2e-native/lib/harness.mjs`
- Create: `e2e-native/tests/files-resource-native.test.mjs`
- Modify: `docs/guide/explorer.md`
- Modify: `docs/guide/workbench.md`
- Modify: `docs/developer/explorer-sidebar.md`
- Modify: `docs/developer/workbench-surfaces.md`
- Modify: `docs/developer/tauri-command-reference.md`
- Modify: `docs/developer/ipc-events.md`
- Create: `e2e/screenshots/files-surface/.gitkeep`

**Interfaces:**
- Consumes: real Tauri file commands, shared test `WARDIAN_HOME`, mock agent configs, and Workbench persistence.
- Produces: native evidence for authorization/watch/ticket behavior and cross-platform documentation for the still-reserved Files foundation.

- [ ] **Step 1: Add a failing native authorization/watch scenario**

Create a temp workspace, additional directory, system include, exact picker fixture, and escape link where supported. Assert primary/additional reads pass, system/escape/sibling reads fail, stable rewrite increments once, stale revision fails, byte range is correct, and closing both subscriptions removes the watcher.

- [ ] **Step 2: Run the targeted native test**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/files-resource-native.test.mjs`

Expected before implementation wiring is complete: the first `open_file_resource` call fails as unknown or unavailable.

- [ ] **Step 3: Complete harness commands and make the native test pass**

Expose only reusable helpers for invoking Files commands and waiting for `file-resource://revision`. Keep temp roots under the harness-created directory and resolve them before cleanup.

- [ ] **Step 4: Document the foundation honestly**

Update Explorer and Workbench docs to explain transient versus permanent tabs, renderer limits, exact native-picker grants, and that the Files launcher remains unavailable until artifact review and isolation ship. Document every new command/event with request and response JSON using `<absolute-workspace-path>` placeholders and POSIX examples before PowerShell examples.

- [ ] **Step 5: Run the slice verification suite**

Run: `npm run lint`

Run: `npm run test`

Run: `npm run build`

Run: `npm run test:e2e -- e2e/tests/files-surface-foundation.spec.ts`

Run: `cargo check --workspace`

Run: `cargo clippy --workspace -- -D warnings`

Run: `cargo test --workspace -- --test-threads=1`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/files-resource-native.test.mjs`

Expected: every command passes. Files remains absent or disabled in New Surface.

- [ ] **Step 6: Capture PR evidence and commit docs/tests**

Capture the transient-to-permanent interaction under `e2e/screenshots/files-surface/<timestamp>/`, upload it through the GitHub PR attachment flow, and embed the HTTPS image in the PR description.

```bash
git add e2e-native e2e docs/guide docs/developer
git commit -m "test(files): prove file surface foundation"
```
