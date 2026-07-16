# Live Artifact Isolation and Files Activation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Safely render live HTML and active SVG, harden retention/performance/recovery and desktop-only behavior, connect remaining file entry points, then activate the complete Files contribution.

**Architecture:** A fail-closed native security proof selects a live-document host only if adversarial content cannot reach Wardian privileges or any network. Rust issues short-lived artifact/version-scoped capability URLs and serves canonical authorized local dependencies under a restrictive document CSP. The Files controller owns renderer leases and restoration while backend GC retains referenced blobs. Activation is a final isolated commit after native denial, lifecycle, full-suite, documentation, and screenshot gates pass.

**Tech Stack:** Rust, Tauri 2 ACL/custom protocols or child webviews, Axum test sentinels, React 19, sandboxed browser primitives, Monaco, PDF.js, Vitest, Playwright, Wardian native E2E, and VitePress.

## Global Constraints

- Plans 1–3 must be merged first. This plan may refine their interfaces but may not bypass authorization, revision, artifact, checkpoint, draft, or review services.
- Live HTML and active SVG are required release behavior. Source-only fallback does not satisfy activation.
- Artifact scripts may run only inside the artifact document. They receive no parent DOM, Tauri IPC, arbitrary filesystem, shell, clipboard, persistent storage, popup, download, top-navigation, or form-submission privilege.
- Network access is never allowed in v1. There is no per-artifact allow button.
- Local dependencies are canonicalized and must remain in the originating agent's `folder` or `include_directories`; `system_include_directories` remain excluded.
- Every capability is unguessable, artifact/version scoped, least-privilege, short-lived, revocable, and checked on each request.
- Passive Markdown remains in the trusted Wardian renderer with raw HTML disabled. Markdown containing active embedded HTML crosses the same isolation boundary as HTML.
- Image/PDF parsing remains isolated and limit-checked; executable documents never reuse the ordinary `wardian-resource` read ticket.
- Renderer crash, hang, memory failure, malformed content, or missing dependency remains local to one Files tab.
- Hidden Files surfaces release expensive renderer instances after the existing Workbench suspension grace period.
- Remote/mobile Files is unsupported in v1 and must not mount Monaco/PDF/live hosts, mutate files, or request terminal takeover.
- `CORE_SURFACE_CONTRIBUTIONS` stays `reserved: true` until Task 6. If any security or native behavior gate fails, do not activate Files.

---

### Task 1: Build the adversarial live-host proof before choosing the shipping host

**Files:**
- Create: `src/features/files/live/LiveArtifactHost.ts`
- Create: `src/features/files/live/SandboxedArtifactFrameHost.tsx`
- Create: `src/features/files/live/SandboxedArtifactFrameHost.test.tsx`
- Create: `src-tauri/src/live_artifacts/mod.rs`
- Create: `src-tauri/src/live_artifacts/security_probe.rs`
- Modify: `src-tauri/src/lib.rs`
- Create: `e2e-native/fixtures/live-artifacts/adversarial.html`
- Create: `e2e-native/fixtures/live-artifacts/active.svg`
- Create: `e2e-native/tests/live-artifact-isolation-native.test.mjs`
- Modify: `e2e-native/lib/harness.mjs`
- Create: `docs/developer/live-artifact-isolation.md`

**Interfaces:**
- Consumes: current `withGlobalTauri: true`, current main-window Tauri capability, a loopback sentinel owned by the native test, and artifact host lifecycle callbacks.
- Produces: `LiveArtifactHost`, a complete denial matrix, a documented host decision, and a non-negotiable activation gate.

- [ ] **Step 1: Add the failing adversarial native test first**

The fixture attempts all of these and reports results through a one-way test-only visual/status channel:

- read `window.__TAURI__` and `window.__TAURI_INTERNALS__`;
- invoke a test Tauri command and core window/event APIs;
- read or mutate `window.parent.document` and top location;
- read/write localStorage, sessionStorage, IndexedDB, and cookies;
- use Clipboard API;
- request a loopback sentinel with `fetch`, XHR, WebSocket, EventSource, `<img>`, `<script>`, stylesheet, font, media, and nested frame;
- submit a form, assign `location`, call `window.open`, create a download, and top-navigate;
- request an authorized sibling, unauthorized parent file, traversal path, absolute path, stale token, and another artifact's token;
- continue executing local inline script and a permitted local dependency.

The sentinel records every inbound request so a UI-side `catch` is not accepted as proof of no network attempt.

- [ ] **Step 2: Run the native proof against an intentionally permissive fixture**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/live-artifact-isolation-native.test.mjs`

Expected: the permissive fixture fails multiple denial assertions. Record the exact failures in the test output; do not weaken assertions.

- [ ] **Step 3: Implement the primary sandbox-frame candidate**

`SandboxedArtifactFrameHost` uses an iframe with `sandbox="allow-scripts"` only: no `allow-same-origin`, popups, forms, downloads, modals, pointer lock, presentation, or top navigation. Its document comes from a backend-issued unique capability URL and includes a response CSP at least as strict as:

```text
default-src 'none';
script-src 'unsafe-inline' wardian-artifact:;
style-src 'unsafe-inline' wardian-artifact:;
img-src wardian-artifact: data:;
font-src wardian-artifact:;
media-src wardian-artifact:;
connect-src 'none';
frame-src 'none';
worker-src 'none';
object-src 'none';
base-uri 'none';
form-action 'none';
```

Omit `allow-same-origin` so storage APIs fail with an opaque origin. Parent UI receives only backend lifecycle state; artifact `postMessage` is ignored in v1.

- [ ] **Step 4: Run the full denial matrix on every native CI platform**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/live-artifact-isolation-native.test.mjs`

Expected shipping result: permitted inline/local behavior passes and every privileged/network attempt is absent from both artifact results and sentinel logs.

- [ ] **Step 5: Apply the fail-closed host selection rule**

If the sandbox frame passes on Windows WebView2, macOS WKWebView, and Linux WebKitGTK, keep it and document the passing evidence. If any platform exposes Tauri globals/IPC, sends a sentinel request, permits navigation, or permits unauthorized file access, implement a capability-free child webview in `src-tauri/src/live_artifacts/child_webview.rs` and `src/features/files/live/ChildWebviewArtifactHost.tsx` with these exact rules:

- window/webview labels begin `artifact-` and match no entry in `src-tauri/capabilities/default.json`;
- the child loads only the backend capability origin;
- Tauri `on_navigation` rejects every URL outside that exact origin before request dispatch;
- download and new-window callbacks always reject;
- backend commands create, position, resize, hide, show, and destroy the child from pane bounds;
- suspension destroys the child and revokes its capability;
- the same denial matrix must pass before it can replace the frame host.

Delete the non-shipping host implementation after the decision, retain its test fixture, and record the selected host plus evidence in `docs/developer/live-artifact-isolation.md`. Do not carry a runtime “try insecure host then fall back” path.

- [ ] **Step 6: Add unit tests for host lifecycle containment**

Assert capability issue on mount, revocation on source change/unmount/suspend, no artifact message handler, crash-to-resource-error transition, Reset Renderer issuing a fresh capability, and link clicks routed to a trusted parent confirmation without navigation.

- [ ] **Step 7: Commit only after the denial proof passes**

```bash
git add src/features/files/live src-tauri/src/live_artifacts src-tauri/src/lib.rs e2e-native/fixtures/live-artifacts e2e-native/tests/live-artifact-isolation-native.test.mjs e2e-native/lib/harness.mjs docs/developer/live-artifact-isolation.md
git commit -m "feat(security): isolate live artifact documents"
```

### Task 2: Artifact capability broker and networkless local dependency serving

**Files:**
- Create: `crates/wardian-core/src/artifacts/capability.rs`
- Modify: `crates/wardian-core/src/artifacts/mod.rs`
- Test: `crates/wardian-core/src/artifacts/capability.rs`
- Create: `src-tauri/src/live_artifacts/capability_broker.rs`
- Create: `src-tauri/src/live_artifacts/protocol.rs`
- Modify: `src-tauri/src/live_artifacts/mod.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Create: `src-tauri/src/commands/live_artifacts.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/live_artifacts/capability_broker.rs`
- Test: `src-tauri/src/live_artifacts/protocol.rs`

**Interfaces:**
- Consumes: `AuthorizedRootService`, artifact/version manifest, source hash, renderer leases, canonical dependency requests, and the selected Task 1 host.
- Produces: `ArtifactCapabilityBroker`, `ArtifactCapabilityV1`, `issue_live_artifact_capability`, `revoke_live_artifact_capability`, and `wardian-artifact` document/dependency responses.

- [ ] **Step 1: Add failing capability tests**

Cover entry document, relative script/style/image/font/media, nested relative paths, percent-encoded traversal, mixed separators, symlink/junction escape, absolute paths, system include, another artifact/version, expiry, revocation, one renderer lease ending, MIME sniffing, range behavior, and network-looking URLs.

```rust
assert_eq!(
    broker.resolve(&token, "../outside.js").unwrap_err().code(),
    "artifact_dependency_unauthorized",
);
```

- [ ] **Step 2: Run focused tests and confirm the broker is missing**

Run: `cargo test -p wardian-core artifacts::capability -- --test-threads=1`

Run: `cargo test -p Wardian live_artifacts -- --test-threads=1`

Expected: missing module/type failures.

- [ ] **Step 3: Define least-privilege capability records**

Each capability binds token hash, artifact ID, version/source hash, canonical entry file, authorized roots snapshot, allowed MIME families, issued/expiry timestamps, renderer lease ID, and revoked state. Raw tokens never persist. Default lifetime is five minutes and renews only while the same visible renderer lease remains active.

- [ ] **Step 4: Resolve every dependency through canonical authorization**

Decode once, reject NUL/absolute/scheme/network-looking paths, join relative to the entry document, canonicalize the existing target, then authorize against the captured roots. Do not grant the entry's parent directory by string prefix. Revalidate the agent's current roots before serving so revoked grants take effect.

- [ ] **Step 5: Serve strict document and asset responses**

The entry response injects the fixed security CSP as an HTTP/header policy before content. Preserve inline script/style but do not rewrite security headers from the artifact. Asset responses set exact MIME, `X-Content-Type-Options: nosniff`, no-store cache, no cookies, no CORS wildcard, and no redirect. Reject `http:`, `https:`, `ws:`, `wss:`, `file:`, `data:` for scripts, and protocol-relative URLs. Allow `data:` only for image data already embedded in the document CSP.

- [ ] **Step 6: Add issue/revoke commands and lease cleanup**

The frontend requests a capability by artifact/version or authorized file resource plus current renderer lease. Backend returns only the entry URL and expiry. Suspension, tab close, renderer reset, root revocation, version change, or app exit revokes the capability immediately.

- [ ] **Step 7: Run core, Tauri, and native denial tests**

Run: `cargo test -p wardian-core artifacts::capability -- --test-threads=1`

Run: `cargo test -p Wardian live_artifacts -- --test-threads=1`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/live-artifact-isolation-native.test.mjs`

Expected: capability scope and every denial assertion pass.

- [ ] **Step 8: Commit the capability broker**

```bash
git add crates/wardian-core/src/artifacts src-tauri/src/live_artifacts src-tauri/src/state/app_state.rs src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(security): broker local artifact dependencies"
```

### Task 3: Live HTML/SVG renderer and active Markdown boundary

**Files:**
- Create: `src/features/files/renderers/LiveArtifactRenderer.tsx`
- Create: `src/features/files/renderers/LiveArtifactRenderer.test.tsx`
- Create: `src/features/files/renderers/liveArtifactPolicy.ts`
- Create: `src/features/files/renderers/liveArtifactPolicy.test.ts`
- Modify: `src/features/files/renderers/MarkdownRenderer.tsx`
- Modify: `src/features/files/renderers/MarkdownRenderer.test.tsx`
- Modify: `src/features/files/rendererRegistry.ts`
- Modify: `src/features/files/rendererRegistry.test.ts`
- Modify: `src/features/files/FilesSurface.tsx`
- Test: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: selected live host, capability issue/revoke commands, HTML/SVG descriptors, renderer lifecycle, Monaco source diff/Draft, and trusted link confirmation.
- Produces: live Preview for HTML/active SVG, isolated active Markdown fragments, source Changes/Draft, renderer reset, and missing-dependency errors.

- [ ] **Step 1: Add failing renderer policy tests**

Assert HTML and active SVG resolve to `live-artifact`, Preview mounts the selected host, Changes uses source Monaco diff, Draft uses source Monaco editor, ordinary Markdown stays passive, Markdown with allowed active embedded HTML uses the live boundary, local links are intercepted, external links require trusted confirmation, and suspension revokes/destroys the host.

- [ ] **Step 2: Run focused renderer tests**

Run: `npm run test -- --run src/features/files/renderers/LiveArtifactRenderer.test.tsx src/features/files/renderers/liveArtifactPolicy.test.ts src/features/files/renderers/MarkdownRenderer.test.tsx src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.test.tsx`

Expected: HTML/SVG still select `UnsupportedRenderer`.

- [ ] **Step 3: Implement live Preview without source-only fallback**

Issue a capability only while Preview is visible, pass the entry URL to the selected host, show loading/ready/crashed/missing-dependency/expired states, and offer Reset Renderer. Never render artifact HTML with `dangerouslySetInnerHTML` in the Wardian document.

- [ ] **Step 4: Keep editing and diffs in trusted Monaco**

Changes for HTML/SVG compares source from checkpoint/presented version. Draft edits source and uses the existing three-way merge/apply/review services. Preview after draft edits uses a draft-scoped immutable blob/capability, not unsaved text injected into parent DOM.

- [ ] **Step 5: Define active Markdown explicitly**

Default Markdown remains `react-markdown` with raw HTML disabled. If a file/artifact is explicitly marked active by descriptor metadata or contains an approved active-artifact wrapper, compile the full Markdown output backend-side to an immutable HTML blob and render it through the live host. Do not enable arbitrary raw HTML in the trusted Markdown tree.

- [ ] **Step 6: Run focused, build, and native tests**

Run: `npm run test -- --run src/features/files/renderers src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.test.tsx`

Run: `npm run build`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/live-artifact-isolation-native.test.mjs`

Expected: live behavior works, workers/assets are local, and the denial matrix stays green.

- [ ] **Step 7: Commit live renderers**

```bash
git add src/features/files/renderers src/features/files/rendererRegistry.ts src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.tsx src/features/files/FilesSurface.test.tsx
git commit -m "feat(files): render live html and svg artifacts"
```

### Task 4: Retention, bounded caches, suspension, and resource-local recovery

**Files:**
- Create: `crates/wardian-core/src/artifacts/gc.rs`
- Modify: `crates/wardian-core/src/artifacts/mod.rs`
- Modify: `crates/wardian-core/src/artifacts/store.rs`
- Test: `crates/wardian-core/src/artifacts/gc.rs`
- Create: `src-tauri/src/state/file_resource_cache.rs`
- Modify: `src-tauri/src/state/file_resources.rs`
- Modify: `src-tauri/src/state/artifact_runtime.rs`
- Modify: `src-tauri/src/state/app_state.rs`
- Create: `src-tauri/src/commands/artifact_maintenance.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilePreview.tsx`
- Modify: `src/features/files/FilesSurface.test.tsx`
- Test: `src-tauri/src/state/file_resource_cache.rs`

**Interfaces:**
- Consumes: manifests, reviews, comments, drafts, checkpoints, recent/open resource leases, renderer lifecycle, and configured limits.
- Produces: reference-aware GC, 30-day/2-GiB defaults, bounded LRU caches, renderer lease synchronization, suspension cleanup, and typed recovery actions.

- [ ] **Step 1: Add failing retention/cache/recovery tests**

Cover every retaining reference, oldest eligible eviction, no eviction during manifest transaction/stream lease, explicit closed-thread delete, soft-budget behavior, open/recent surface lease, cache eviction order, hidden renderer suspension, watcher count stability, malformed manifest, deleted/moved file, revoked root, renderer crash, and several concurrent Files panes.

- [ ] **Step 2: Run focused tests**

Run: `cargo test -p wardian-core artifacts::gc -- --test-threads=1`

Run: `cargo test -p Wardian file_resource_cache -- --test-threads=1`

Run: `npm run test -- --run src/features/files/FilesSurface.test.tsx`

Expected: GC/cache types and several recovery states are missing.

- [ ] **Step 3: Implement reference-aware GC**

Retain blobs referenced by active threads, open reviews, unresolved comments, drafts, selected checkpoints, open/restored/recent tabs, active streams, and manifest transactions. Closed unreferenced data becomes eligible after 30 days. A 2 GiB soft budget evicts oldest eligible blobs first. GC acquires a store lease and never deletes a referenced or open blob.

- [ ] **Step 4: Add bounded resource caches**

Cache text/Monaco models by content hash, PDF documents/pages by version hash, and live capabilities by renderer lease. Configure byte/count ceilings in one backend settings struct. Visible panes have priority; hidden panes release renderer instances after Workbench suspension grace while retaining lightweight controller state.

- [ ] **Step 5: Complete typed resource-local recovery**

Map errors to unauthorized/revoked, missing/moved, changed-since-base, unsupported encoding/renderer, oversized, missing dependency, baseline unavailable, renderer crash/resource limit, and manifest/version unavailable. Actions are Retry, Locate File, Keep Draft, Discard Draft, Compare Presented Version, Open With, Reveal, Reset Renderer, and Close. None throws to the fatal App boundary.

- [ ] **Step 6: Add explicit delete and settings commands**

Users may configure retention days and soft bytes and explicitly delete only a closed thread after confirmation. Reject delete for active/open/referenced threads with a typed reason.

- [ ] **Step 7: Run focused and stress tests**

Run: `cargo test -p wardian-core artifacts::gc -- --test-threads=1`

Run: `cargo test -p Wardian file_resource_cache -- --test-threads=1`

Run: `npm run test -- --run src/features/files`

Expected: retention, cache, lease, suspension, and recovery tests pass.

- [ ] **Step 8: Commit lifecycle hardening**

```bash
git add crates/wardian-core/src/artifacts src-tauri/src/state src-tauri/src/commands src-tauri/src/lib.rs src/features/files
git commit -m "feat(files): harden renderer lifecycle and retention"
```

### Task 5: Desktop-only guard and contextual file/artifact entry points

**Files:**
- Create: `src/features/files/FilesUnsupportedState.tsx`
- Create: `src/features/files/FilesUnsupportedState.test.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/remote/RemoteMobileApp.tsx`
- Modify: `src/features/remote/RemoteMobileApp.test.tsx`
- Modify: `src/features/terminal/terminalLinks.ts`
- Modify: `src/features/terminal/terminalLinks.test.ts`
- Modify: `src/features/grid/markdown/markdownSafety.ts`
- Modify: `src/features/grid/markdown/markdownSafety.test.ts`
- Modify: `src/views/WorkflowMonitorView.tsx`
- Modify: `src/views/WorkflowMonitorView.test.tsx`
- Modify: `src/views/App.tsx`

**Interfaces:**
- Consumes: native runtime detection, Files navigation requests, terminal/chat Markdown links, workflow output paths/artifact IDs, and remote/mobile presentation context.
- Produces: non-mutating unsupported state and trusted contextual open/open-to-side actions.

- [ ] **Step 1: Add failing desktop-boundary tests**

Assert browser/remote/mobile shows title/type/desktop requirement, does not call any Files command, does not mount Monaco/PDF/live host, does not request takeover, and preserves surface state. Assert desktop file/artifact links route through NavigationService and external URLs retain existing safety behavior.

- [ ] **Step 2: Run focused tests**

Run: `npm run test -- --run src/features/files/FilesUnsupportedState.test.tsx src/features/remote/RemoteMobileApp.test.tsx src/features/terminal/terminalLinks.test.ts src/features/grid/markdown/markdownSafety.test.ts src/views/WorkflowMonitorView.test.tsx`

Expected: Files has no explicit unsupported or contextual routing path.

- [ ] **Step 3: Guard before renderer/client construction**

At the top of Files surface resolution, detect a native desktop context. Unsupported contexts render only bounded surface state and descriptive text; they must not construct file clients or hooks that invoke native commands. Remote routes never offer Take control for Files.

- [ ] **Step 4: Route trusted local links**

Terminal/chat/workflow link parsing recognizes canonical local file paths and `artifact:<id>` references. Trusted UI offers Open and Open to Side and calls Workbench navigation. Do not let artifact-rendered links call NavigationService directly; they pass through the host's trusted confirmation channel.

- [ ] **Step 5: Run focused tests**

Run: `npm run test -- --run src/features/files/FilesUnsupportedState.test.tsx src/features/remote/RemoteMobileApp.test.tsx src/features/terminal/terminalLinks.test.ts src/features/grid/markdown/markdownSafety.test.ts src/views/WorkflowMonitorView.test.tsx`

Expected: desktop routing and zero-invoke unsupported behavior pass.

- [ ] **Step 6: Commit platform and entry-point behavior**

```bash
git add src/features/files src/features/remote src/features/terminal src/features/grid/markdown src/views/WorkflowMonitorView.tsx src/views/WorkflowMonitorView.test.tsx src/views/App.tsx
git commit -m "feat(files): route trusted desktop file entry points"
```

### Task 6: Activate Files in New Surface only after the full contract is green

**Files:**
- Modify: `src/features/workbench/coreSurfaceRegistry.ts`
- Modify: `src/features/workbench/coreSurfaceRegistry.test.ts`
- Modify: `src/features/workbench/OpenSurfaceDialog.tsx`
- Modify: `src/features/workbench/OpenSurfaceDialog.test.tsx`
- Modify: `src/features/workbench/HomeSurface.tsx`
- Modify: `src/features/workbench/HomeSurface.test.tsx`
- Modify: `src/layout/workbench/WorkbenchHost.tsx`
- Modify: `src/layout/workbench/WorkbenchHost.test.tsx`
- Modify: `e2e/fixtures/workbenchIpcMock.ts`
- Create: `e2e/tests/files-surface-complete.spec.ts`
- Create: `e2e-native/tests/files-surface-complete-native.test.mjs`

**Interfaces:**
- Consumes: native picker exact-file capability, complete Files definition/renderers/review contract, New Surface home/dialog, and every native proof from Plans 1–4.
- Produces: enabled **Files** card, picker-to-tab flow, full browser/native release scenarios, and the only change from `reserved: true` to active.

- [ ] **Step 1: Add failing activation tests before changing the contribution**

Assert the card is labeled Files with a file-type icon and concise description; clicking it opens the native picker, cancel leaves the New Tab intact, selection replaces the placeholder with a permanent file tab, and the plus/New Surface flow never opens a modal command palette. Assert internal `files` requests still require a resource key.

- [ ] **Step 2: Run focused activation tests and confirm Files remains disabled**

Run: `npm run test -- --run src/features/workbench/coreSurfaceRegistry.test.ts src/features/workbench/OpenSurfaceDialog.test.tsx src/features/workbench/HomeSurface.test.tsx src/layout/workbench/WorkbenchHost.test.tsx`

Expected: Files is still reserved and cannot be launched.

- [ ] **Step 3: Add a resource-resolving launcher action**

Extend surface contributions with `launch_behavior: "direct" | "pick_file"`. Files uses `pick_file`; Home/OpenSurfaceDialog invoke the injected picker, then call `open_from_placeholder` with `file:<canonical-path>` and a permanent Preview state. Cancel closes only the picker and preserves focus/placeholder.

- [ ] **Step 4: Make the activation change in isolation**

Change the contribution to:

```ts
{
  surface_type: "files",
  title: "Files",
  description: "Inspect, compare, edit, and review local files.",
  group: "Core views",
  launch_behavior: "pick_file",
}
```

Remove `reserved: true`. Do not add a separate Artifacts card.

- [ ] **Step 5: Add complete browser E2E**

Cover New Surface picker flow, Explorer transient/permanent/split, text/Markdown/image/PDF/live HTML/SVG, Changes/Draft, conflict, review drawer, Queue/Quick Open, responsive overlay, renderer failure, and unsupported mocked browser/mobile state. Use mocks only for native boundaries.

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/files-surface-complete.spec.ts`

Expected: complete browser behavior passes.

- [ ] **Step 6: Add complete native E2E**

Compose the targeted native helpers to prove real picker grant, authorized roots, stable watch, CLI presentation/reuse, restart, checkpoint diff, apply/conflict, review retrieval, live denial, renderer suspension, multi-pane stability, and app-not-running error in one release smoke.

Run: `npm run test:e2e:native:fast -- e2e-native/tests/files-surface-complete-native.test.mjs`

Expected: release smoke passes without terminal takeover or fatal UI errors.

- [ ] **Step 7: Commit activation separately**

```bash
git add src/features/workbench src/layout/workbench e2e/fixtures/workbenchIpcMock.ts e2e/tests/files-surface-complete.spec.ts e2e-native/tests/files-surface-complete-native.test.mjs
git commit -m "feat(workbench): activate complete files surface"
```

### Task 7: Documentation, screenshots, issue evidence, and final repository gate

**Files:**
- Modify: `docs/guide/workbench.md`
- Modify: `docs/guide/explorer.md`
- Modify: `docs/guide/queue.md`
- Modify: `docs/guide/cli.md`
- Modify: `docs/guide/artifacts.md`
- Modify: `docs/developer/workbench-surfaces.md`
- Modify: `docs/developer/live-artifact-isolation.md`
- Modify: `docs/developer/tauri-command-reference.md`
- Modify: `docs/developer/ipc-events.md`
- Modify: `docs/developer/state-management.md`
- Modify: `docs/developer/theming.md`
- Create: `e2e/screenshots/files-surface/<timestamp>/files-review-wide.png`
- Create: `e2e/screenshots/files-surface/<timestamp>/files-review-narrow.png`
- Create: `e2e/screenshots/files-surface/<timestamp>/live-html.png`

**Interfaces:**
- Consumes: shipped UI, exact command/event schemas, selected host proof, repository PR template, and issues #392/#393/#395.
- Produces: complete user/developer guidance, HTTPS screenshot evidence, verification logs, and issue/PR handoff.

- [ ] **Step 1: Update docs to the shipped contract**

Document Files opening/pinning/splits, Preview/Changes/Draft, renderer limits, live networkless behavior, authorized roots/native picker, artifact CLI, versions/working drift, comments/approval, Apply versus Send, conflicts, Queue/Quick Open, retention/settings, typed recovery, desktop-only support, and security architecture. Use placeholders, POSIX examples first, and labeled PowerShell alternatives.

- [ ] **Step 2: Capture feature-specific screenshots**

Capture a wide review drawer, narrow overlay drawer, and live HTML artifact with surrounding Files controls. Store under the timestamped path, upload through GitHub, and embed at least one HTTPS image in the PR body. Do not use empty-window or app-tour screenshots.

- [ ] **Step 3: Run all frontend validation**

Run: `npm run lint`

Run: `npm run test`

Run: `npm run build`

Run: `npm run test:e2e`

Run: `npm run docs:build`

Run: `npm run check:frontend-screenshot`

Expected: every command passes.

- [ ] **Step 4: Run all Rust validation**

Run: `cargo check --workspace`

Run: `cargo clippy --workspace -- -D warnings`

Run: `cargo test --workspace -- --test-threads=1`

Expected: every command passes.

- [ ] **Step 5: Run every targeted native proof**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/files-resource-native.test.mjs`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-lifecycle-native.test.mjs`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/artifact-review-native.test.mjs`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/live-artifact-isolation-native.test.mjs`

Run: `npm run test:e2e:native:fast -- e2e-native/tests/files-surface-complete-native.test.mjs`

Expected: authorization, lifecycle, review, isolation, and complete native smoke pass.

- [ ] **Step 6: Inspect repository safety**

Run: `git status --short`

Run: `git diff --check origin/main...HEAD`

Run: `git diff --name-only origin/main...HEAD`

Expected: no whitespace errors, secrets, `.env` files, generated native-driver artifacts, or unrelated changes.

- [ ] **Step 7: Update issues and prepare the PR**

Link #392 and note #393/#395 coverage. Use the PR template, explain why the surface is unified, include exact verification logs, document the security host decision, and embed the HTTPS screenshot. Do not claim real-provider coverage unless the opt-in test was actually run.

- [ ] **Step 8: Commit final documentation**

```bash
git add docs/guide docs/developer e2e/screenshots/files-surface
git commit -m "docs(files): document artifact review surface"
```
