# Files and Artifact Review Surface

- **Status:** Approved
- **Date:** 2026-07-16
- **Primary issue:** [#392](https://github.com/wardian-app/Wardian/issues/392)
- **Related issues:** [#393](https://github.com/wardian-app/Wardian/issues/393), [#395](https://github.com/wardian-app/Wardian/issues/395), [#513](https://github.com/wardian-app/Wardian/issues/513)
- **Depends on:** `docs/specs/2026-07-10-workbench-navigation-system.md`

## Context and Problem Statement

Wardian agents increasingly create code, Markdown, reports, images, PDFs, HTML
applications, and other files that users need to inspect without leaving the
agent or workflow in front of them. The current Explorer preview is a modal
`<pre>` backed by an unbounded `read_to_string` command. Terminal and chat file
links generally open an external editor. Source Control has a separate raw diff
modal. None of these paths supports durable tabs, artifact provenance, prompt-
scoped changes, comments, or a safe user-edit/agent-review loop.

The Workbench established by #513 provides the correct navigation substrate:
resource-keyed tabs, groups, splits, restoration, badges, Quick Open, and a
surface registry. The next contribution should use that substrate rather than
introduce a second document window, a nested tab system, or separate viewers
for files and agent artifacts.

Wardian needs one **Files** surface that is familiar to users of VS Code while
also supporting agent-native artifact presentation and review. It must preserve
the distinction between a mutable file and a versioned review thread, allow
users to send proposed changes without first mutating disk, and show exactly
what changed since Wardian delivered the last user prompt without claiming
exclusive agent authorship.

## Goals

1. Open ordinary files and agent-presented artifacts as first-class Workbench
   tabs that can participate in groups, splits, Quick Open, and restoration.
2. Render text/code with Monaco, Markdown, images, PDFs, live HTML, and active
   SVG in-app using renderer-specific isolation and performance limits.
3. Default to inspection. Editing is an explicit Draft mode with independent
   **Apply to file** and **Send to agent** outcomes.
4. Support versioned artifact review with provenance, line/range or spatial
   comments, general notes, approval, and re-presentation into the same thread.
5. Show a line-by-line **Changes since prompt** view for indexed text files and
   a version comparison for images and PDFs.
6. Establish one prompt change-tracking service that Explorer can later use for
   all files instead of building an artifact-only diff engine.
7. Respect the exact directory roots granted to the originating agent and keep
   executable artifact content isolated from Wardian application privileges.
8. Remain stable in multiple panes, under live file writes, after restart, and
   when a renderer or backing file fails.

## Non-Goals

- A mobile or remote Files experience in the first release. Those clients show
  a clear unsupported state and do not attempt to run Monaco.
- Office document, spreadsheet, presentation, notebook, audio, or video
  rendering in the first release.
- Semantic or pixel-level image/PDF diffing.
- Executing HTML with Wardian/Tauri privileges or allowing artifact network
  access.
- A separate Artifacts collection surface. Queue attention, Quick Open, and
  recent history provide initial rediscovery.
- A provider-specific parser that guesses which files an agent changed.
- Proving that a temporally changed line was authored by the agent.
- Blocking an agent runtime until every artifact is approved.
- Replacing Source Control's repository-wide staging and commit workflows.
- General multi-pane placement customization. The first release accepts
  semantic presentation intent and leaves richer placement policy for later.

## Product Vocabulary

| Term | Meaning |
|---|---|
| File resource | Mutable local file identified by canonical path. |
| Artifact thread | Provenance-bearing review object identified by `artifact_id` and backed by a local file. |
| Presented version | Immutable snapshot created by an explicit `wardian artifact present`. |
| Working state | Current bytes of the backing file, which may differ from the last presented version. |
| Prompt checkpoint | Durable content-index root captured immediately before Wardian delivers a user prompt. |
| Draft | User-authored proposed text based on a specific file hash; it does not mutate disk. |
| Review | Batched draft patch, anchored comments, and general note sent to the originating agent. |
| Authorized roots | The agent's primary workspace plus its user-granted `include_directories`. |

## Proposed Decision

### One Files surface, two resource identities

Replace the reserved `file-editor` contribution with a registered `files`
surface shown as **Files** in New Surface. The surface uses
`focus_resource`, `suspend_when_hidden`, and `confirm_if_dirty` policies.

The surface hosts two explicit resource kinds:

```ts
type FileResourceKey = `file:${string}`;       // canonical normalized path
type ArtifactResourceKey = `artifact:${string}`; // stable artifact_id

type FilesSurfaceStateV1 = {
  resource_kind: "file" | "artifact";
  mode: "preview" | "changes" | "draft";
  transient_preview: boolean;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
};
```

Only bounded presentation state is serialized in the Workbench document.
Contents, drafts, comments, versions, capabilities, and watcher state live in
backend-owned stores and are referenced by ID.

The same canonical path may intentionally be open once as an ordinary file and
again as an artifact thread. The file tab means “show the current mutable file.”
The artifact tab means “show this versioned review relationship with this
origin.” This avoids making tab identity change when an agent presents a file
that the user already has open.

### Component boundaries

```text
FilesSurface
├── FilesResourceController       loads file/artifact state by resource key
├── FilesModeBar                  Preview / Changes / Draft and primary actions
├── RendererRegistry              content descriptor -> renderer contribution
├── ReviewDrawer                  provenance, versions, comments, status
└── FilesWorkbenchAdapter         title, icon, badges, close guard, restoration

Rust backend
├── AuthorizedRootService         canonical agent-readable roots
├── FileResourceService           descriptors, streaming, watching, atomic write
├── ArtifactStore                 threads, versions, reviews, drafts, snapshots
├── PromptChangeTracker           content index and prompt checkpoints
├── DraftMergeService             base/current/draft merge and conflicts
├── ArtifactControlService        CLI/control protocol and agent delivery
└── ArtifactCapabilityBroker      renderer-scoped local resource access
```

The renderer registry receives an immutable content descriptor and capability
set. It does not know how artifact threads are versioned or how prompts are
delivered. The review services do not import Monaco, PDF.js, or Workbench layout
code. Workbench never persists renderer-private state beyond small view fields
such as mode and selected version.

### Opening and tab behavior

Explorer routes file selection through `NavigationService` instead of opening
its current modal.

- Single-click opens one transient file preview in the target group. Opening a
  second file replaces that group's existing transient preview.
- Double-click, Pin, entering Draft, or making a comment converts the preview
  into a permanent tab.
- Open to Side creates or focuses the resource in an adjacent group using the
  Workbench's normal split admission rules.
- An artifact presentation is always permanent, opens in the active group as a
  background tab, and receives an attention badge without stealing focus.
- Re-presenting the same canonical path from the same agent origin reuses its
  active artifact thread. `--new` and `--artifact <id>` override that default.
- Closing an artifact tab closes only its presentation. Queue attention and
  Quick Open/recent history continue to expose the persisted thread.

Tab icons come from the renderer/content type. Tab labels use the basename;
duplicate basenames gain the shortest distinguishing parent path. Artifact
tabs add review/attention badges instead of verbose title prefixes.

### Pane layout

The existing Workbench tab strip remains the only tab strip. The Files content
area begins with one compact mode/action row containing:

- canonical breadcrumb;
- Preview, Changes, and Draft modes;
- review status;
- contextual Approve, Send to agent, and Apply to file actions;
- an overflow menu for metadata, copy path, reveal, Open With, versions, and
  reset/discard actions.

The content renderer consumes the remaining pane. A collapsible review drawer
on the right shows provenance, versions, comments, and review status. Below the
responsive width threshold it becomes an overlay drawer instead of reducing the
renderer below its usable minimum. Every spatial annotation also appears in a
keyboard-accessible list in the drawer.

### Renderer registry and first-release renderers

Content detection uses extension as a hint, then MIME/signature and UTF
validation. The backend returns a typed descriptor and capability flags rather
than raw bytes through normal IPC.

| Content | Preview | Changes | Draft |
|---|---|---|---|
| Text/code/config | Read-only Monaco with language detection | Monaco line diff | Editable Monaco |
| Markdown | Rendered document by default | Monaco source diff | Editable Monaco source |
| Image | Fit/actual size, pan, zoom | Previous/current side by side plus metadata | Not editable |
| PDF | PDF.js pages, search, zoom | Previous/current side by side plus metadata | Not editable |
| HTML | Live isolated document | Source diff; prior/current live preview available | Editable Monaco source |
| Active SVG | Live isolated document | Source diff; prior/current preview available | Editable Monaco source |
| Unsupported/oversized | Metadata and Open With | Unavailable with reason | Unavailable |

The renderer interface is extension-ready:

```ts
type FileRendererDefinition = {
  renderer_id: string;
  matches: (descriptor: FileContentDescriptor) => boolean;
  capabilities: {
    preview: boolean;
    changes: "line" | "version" | "none";
    draft: boolean;
    annotations: "line_range" | "spatial" | "general";
  };
  render: FileRenderer;
};
```

Monaco workers are bundled through Vite and loaded lazily. PDF.js is bundled;
the Files surface does not rely on operating-system WebView PDF behavior.

### Live artifact isolation

HTML and active SVG are expected to render and run. They run as untrusted
documents in a unique isolated origin with zero Tauri capabilities. The
isolation contract is behavioral, regardless of whether its implementation is
a hardened sandboxed frame or a capability-free child webview:

- scripts may manipulate only their artifact document;
- no Wardian parent DOM, Tauri IPC, filesystem, shell, clipboard, persistent
  storage, popup, download, top-navigation, or form-submission privilege;
- no same-origin relationship with the Wardian application;
- `connect-src` and all network-backed subresources are denied;
- local scripts, styles, images, fonts, and media load only through unguessable,
  short-lived capability URLs issued by Rust;
- every local dependency is canonicalized and must remain inside the
  originating agent's authorized roots;
- links are intercepted and offered to trusted Wardian UI instead of navigating
  the artifact host;
- renderer lifecycle, crash, and memory failure are contained to the resource
  tab.

Inline script and style are allowed inside the isolated document because live
HTML artifacts require them. That exception never crosses into Wardian's CSP.
Passive Markdown renders in Wardian's Markdown renderer; active embedded HTML
is hosted across the same isolation boundary. Image decoding and PDF parsing
use isolated workers/viewers and validate resource limits before allocation.
The network prohibition is unconditional in the first release; there is no
per-artifact allow button.

### Agent directory authorization

An agent-presented path is authorized against:

```text
authorized_roots(agent) =
  canonical(agent.folder)
  ∪ canonical_each(agent.include_directories)
```

`system_include_directories` are deliberately excluded. They expose Wardian's
managed instructions, classes, agent homes, and skills to providers and are not
user grants for artifact publication.

The backend owns this root computation and the provider launch adapters consume
the same user-granted directory list. Validation resolves symlinks and Windows
junctions before checking root containment. A link cannot escape an authorized
root unless its resolved destination is independently authorized. Missing or
unresolvable paths fail closed.

Files explicitly selected by the user through a native picker receive a
resource-specific user capability and may open as file resources even when they
are outside the selected agent's roots. That action does not mutate the agent's
configuration, does not let the agent present neighboring files, and does not
promise a prompt checkpoint. Sending such a draft to an agent is disabled until
the file lies within one of that agent's authorized roots.

Apply writes without an additional location confirmation anywhere inside the
appropriate capability: an agent-authorized root or the exact native-picker
file selected by the user.

### Artifact CLI and control protocol

The provider-neutral entry point is:

```text
wardian artifact present <path>
  [--title <title>]
  [--description <markdown>]
  [--artifact <artifact-id> | --new]
  [--address <comment-id>]...
```

The command requires the running desktop app for the same `WARDIAN_HOME` and a
valid `WARDIAN_SESSION_ID`. It returns structured JSON containing
`artifact_id`, `version_id`, `canonical_path`, `reused_thread`, persistence
state, and UI delivery state. `app_not_running`, invalid origin, unauthorized
path, unreadable file, and unstable-file timeout are explicit nonzero failures;
the CLI never prints success before persistence and UI routing complete.

The first release keeps placement semantic: presentation means “open a
background tab and request attention.” It does not expose group IDs or layout
coordinates. Future user policy may route presentations beside their origin
agent or into a dedicated group without changing the CLI contract.

Review retrieval uses:

```text
wardian artifact show <artifact-id> [--version <version-id>]
wardian artifact review show <artifact-id> [--review <review-id> | --latest]
```

`review show` returns the base/current hashes, unified patch, structured
comments, general note, apply state, and prompt checkpoint reference. When a
user sends a review, Wardian delivers a concise attributed message telling the
origin agent which review is available and how to retrieve it. Large patches
are never pasted wholesale into the terminal input channel.

Repeated `--address` flags let an agent explicitly mark comment IDs addressed
when presenting the next version. Wardian records that claim but does not mark
the comments user-resolved.

### Artifact lifecycle

```text
presented
   ├── user approves ───────────────> approved
   └── user sends review ───────────> feedback_sent
                                         │
                              agent re-presents
                                         v
                                      updated
                                         │
                                 review or approve
```

Approval is review state, not an agent-runtime gate. A thread can be closed and
later reopened. Re-presentation creates an immutable version only when the agent
explicitly invokes `artifact present`; file-save events never flood version
history.

The backing file remains live. After a stable-write debounce, Preview updates
to the working state and the artifact shows **Changed since presented** when its
hash differs from the selected version. The selected immutable version remains
available for review and comparison.

Comments have explicit state:

- `open`: awaiting action;
- `agent_marked_addressed`: the presenting agent cited its ID;
- `resolved`: the user confirmed resolution;
- `outdated`: its anchor cannot map safely to the selected/new version.

Wardian does not infer semantic resolution from changed content. Unchanged
anchors may carry forward by exact content identity. Changed line or spatial
anchors remain attached to their original version and become outdated unless a
deterministic mapping succeeds.

### Prompt checkpoints and Changes since prompt

The backend adds one shared `PromptChangeTracker`; Files consumes it first and
Explorer later consumes the complete change set.

For each agent, the tracker incrementally indexes the primary workspace and
user-granted `include_directories`. It excludes `system_include_directories`,
Wardian state, VCS internals, and high-churn dependency/build caches by default.
An explicitly tracked artifact inside an authorized root overrides ordinary
ignore rules for future checkpoints.

Immediately before Wardian delivers a user-originated prompt, including an
artifact review, the backend:

1. flushes stable pending file events for that agent's authorized roots;
2. records the current content-index root and origin metadata;
3. durably associates the resulting `checkpoint_id` with the delivered input;
4. only then submits the input to the provider.

The index is content-addressed, so a checkpoint references immutable hashes
rather than copying unchanged workspaces. Comparing the checkpoint root with
the current index yields added, modified, deleted, and renamed paths without
depending on Git or provider transcript parsing. Git status may enrich the UI
but is not authoritative.

The label is **Changes since prompt**, not “Agent changes.” The checkpoint
establishes temporal scope, not authorship. Ordinary files opened from Explorer
carry the selected agent/checkpoint context when available. Files opened without
an agent context do not invent one.

If indexing was not ready before a prompt, or a native-picker file outside the
authorized roots had not been observed, Wardian shows **Prompt baseline
unavailable**. It may offer comparison to a previous presented version or first
observed state, but it does not relabel that comparison as prompt-scoped.

### Drafts, apply, and review submission

Entering Draft pins a transient tab and creates a durable draft containing the
base content hash, base version/checkpoint references, and user text. Drafts
survive navigation and restart.

When the backing file changes, `DraftMergeService` performs a three-way merge:

```text
base = bytes/hash from draft creation
current = current backing file
proposed = user draft
```

Non-overlapping changes rebase automatically. Overlapping edits enter a Monaco
three-way conflict view. Wardian never silently overwrites current bytes.

**Apply to file** and **Send to agent** are independent:

- Apply repeats the hash/drift check, writes a same-directory temporary file,
  flushes it, atomically replaces the target, and records the applied hash.
- Send persists one immutable review containing the patch, line/range comments,
  spatial comments, and general note, then delivers its reference to the origin
  agent. It never writes the backing file.
- A user may apply, send, do both in either order, or keep the draft local.

Closing a dirty Files presentation uses the Workbench close guard and offers
Keep Draft, Discard Draft, or Cancel. Keeping the durable draft permits the tab
to close without losing work.

### Annotation model

```ts
type ReviewAnchor =
  | {
      kind: "line_range";
      start_line: number;
      start_column: number;
      end_line: number;
      end_column: number;
      context_hash: string;
    }
  | {
      kind: "image_region";
      x_ratio: number;
      y_ratio: number;
      width_ratio: number;
      height_ratio: number;
    }
  | {
      kind: "pdf_region";
      page_index: number;
      x_ratio: number;
      y_ratio: number;
      width_ratio: number;
      height_ratio: number;
      selected_text?: string;
    }
  | { kind: "general" };
```

Coordinates are normalized to the intrinsic image or PDF page, never viewport
pixels. Each anchor includes its `version_id`. Text comments appear in Monaco's
gutter and review drawer. Image/PDF users can drag a region or select PDF text;
the trusted annotation overlay records coordinates outside the untrusted
document host.

### Persistence and retention

Artifact state is backend-owned and inspectable under `<WARDIAN_HOME>`:

```text
artifacts/
├── index.json
├── threads/<artifact-id>/manifest.json
├── reviews/<review-id>.json
├── drafts/<draft-id>.json
├── checkpoints/<checkpoint-id>.json
└── blobs/<sha256>
```

Manifests and reviews use versioned snake_case schemas. Blobs are immutable and
deduplicated. Every manifest/reference write uses the same-directory temp,
flush, atomic-replace, and last-known-good discipline as Workbench persistence.
No layout document contains file bytes or executable artifact state.

Garbage collection is reference-aware:

- active threads, open reviews, unresolved comments, drafts, selected prompt
  checkpoints, and restored/recent tabs retain their blobs;
- closed unreferenced data is eligible after 30 days;
- a 2 GiB default soft budget evicts oldest eligible blobs first;
- users can configure both limits and explicitly delete a closed thread;
- GC never runs while a manifest transaction or renderer stream holds a lease.

### Performance limits and lifecycle

Initial centrally configured limits are:

- Monaco preview/edit: 16 MiB and 200,000 lines;
- Monaco diff: 5 MiB per side and 100,000 lines;
- live HTML/SVG source: 16 MiB, with each brokered dependency checked
  independently;
- images: 64 MiB encoded and 64 megapixels decoded;
- PDF: 256 MiB, streamed/range-read by page rather than copied into React state.

Files over a renderer's safe limit show metadata and Open With. They do not
truncate silently and do not attempt a partial editable model.

The backend owns one reference-counted watcher per canonical path/root.
Components subscribe to stable revisions instead of creating watchers. Hidden
Files surfaces release expensive Monaco/PDF/live-document renderers after the
Workbench suspension grace period while retaining controller state. Visible
surfaces have priority; model/document caches are bounded LRU caches.

Prompt delivery records an already-maintained index root and does not scan a
workspace synchronously. Initial indexing is a separate observable state.

### Error and recovery behavior

All failures are resource-local and typed. A Files tab can show:

- unauthorized or revoked root;
- missing, moved, or unreadable backing file;
- changed-since-base conflict;
- unsupported encoding or renderer;
- oversized input;
- missing local HTML dependency;
- prompt baseline unavailable;
- renderer crashed or exceeded resource limits;
- artifact manifest/version unavailable.

Applicable actions include Retry, Locate File, Keep Draft, Discard Draft,
Compare Presented Version, Open With, Reveal, Reset Renderer, and Close. Locate
or rebind never changes an agent's authorized roots implicitly. A renderer
failure or malformed manifest cannot reach Wardian's fatal application error
boundary.

### Desktop-only first release

Mobile/remote clients display the resource title, type, and a concise message
that Files is currently available on desktop. They do not request takeover,
mount Monaco, run artifact scripts, or mutate the file. The persisted Workbench
tree remains intact so opening the same layout on desktop restores the surface.

## Delivery and Activation

The complete first-release behavior may land through several reviewable PRs,
but the Files card remains disabled until the full contract is present:

1. **File foundation:** backend file descriptors/capabilities, authorized-root
   service, stable watchers, Files registration, transient tabs, renderers, and
   Explorer routing.
2. **Artifact lifecycle:** CLI/control commands, persistent threads/versions,
   background attention, Queue/Quick Open discovery, and review retrieval.
3. **Review tools:** prompt tracker, Monaco changes/drafts, merge conflicts,
   comments, PDF/image annotations, and agent feedback delivery.
4. **Isolation and activation:** live document host security proof, restoration,
   retention, performance/error hardening, docs, screenshots, and default-on
   Files contribution.

These are implementation slices, not user-visible scope tiers. No release
should expose a Files card that silently falls back to the old modal or lacks
the promised review safeguards.

## Testing and Verification

### Frontend unit and integration tests

- File and artifact resource-key resolution and distinct identity.
- Registry presentation, type icons, attention/dirty badges, and close guards.
- Transient preview replacement, pinning, Draft pinning, and Open to Side.
- Renderer selection from descriptors rather than extension alone.
- Preview/Changes/Draft transitions and unavailable-state labels.
- Responsive review drawer and keyboard alternatives for spatial annotations.
- Comment states, version switching, draft persistence, and conflict UI.
- Desktop rendering versus mobile/remote unsupported state.

### Rust unit and integration tests

- Canonical authorized-root calculation uses `folder` plus
  `include_directories` and excludes `system_include_directories`.
- Symlink/junction escape rejection on supported platforms.
- Content detection, stable-write debounce, reference-counted watchers, and
  capability expiry/revocation.
- Atomic apply, base-hash enforcement, clean three-way merge, and overlapping
  conflict output.
- Artifact auto-reuse, explicit new/target thread behavior, schemas, recovery,
  snapshot deduplication, and reference-aware GC.
- Prompt checkpoint ordering before provider submission, index diff behavior,
  ignored-path override, and unavailable baselines.
- Live artifact capability broker denies network, IPC, traversal, navigation,
  unauthorized dependencies, and stale tokens.

### Browser E2E

- Files appears in New Surface and creates a normal Workbench tab.
- Explorer single-click preview, replacement, double-click/pin, Draft pinning,
  split placement, and close behavior using mocked native descriptors.
- Markdown/image/PDF/HTML states, mode controls, comments, versions, review
  drawer, and accessible navigation.
- Narrow panes use an overlay drawer without horizontal control collisions.

Browser E2E does not claim real filesystem, sandbox, watcher, Monaco-worker, or
Tauri control behavior.

### Native E2E

- Real workspace and `include_directories` reads/writes plus rejected
  `system_include_directories` and canonical escapes.
- `wardian artifact present` opens a background tab with attention, reuses the
  correct thread, persists across restart, and fails when the app is absent.
- Real file watching, stable reload, explicit version creation, draft apply,
  conflict handling, and review delivery/retrieval.
- Prompt checkpoint is durable before delivery and produces correct
  added/modified/deleted/untracked file diffs.
- Live HTML executes inside its document while network, Tauri IPC, parent DOM,
  unauthorized local assets, and navigation remain inaccessible.
- Several Files panes, large inputs, renderer suspension, and restore do not
  leak watchers or crash the application.

One opt-in real-provider test may prove the provider-specific last mile:
present, receive structured review, re-present with addressed comment IDs, and
verify the same thread. Provider-specific tests are not required for logic
already proven through native mock-provider coverage.

### Required repository verification

- `npm run lint`
- `npm run test`
- `npm run build`
- `npm run test:e2e`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace -- --test-threads=1`
- `cargo check --workspace`
- targeted native E2E for filesystem, control, checkpoint, and sandbox claims
- feature-specific screenshots under `e2e/screenshots/files-surface/<timestamp>/`
  with at least one representative HTTPS image embedded in every frontend PR

## Documentation and Issue Governance

- #392 is the primary implementation issue and must be updated to point to this
  expanded Files/artifact-review contract.
- #393 owns contextual artifact links from chat, terminal, workflow output, and
  Queue; those entry points route through the same `NavigationService` request.
- #395 owns concrete Queue cards and their deep links into artifact threads.
- The implementation plan should split #392 into the four delivery slices above
  if they will ship as separate PRs.
- `docs/guide/` documents file preview tabs, artifact review, drafts, comments,
  approval, and unsupported mobile behavior.
- `docs/developer/` documents renderer contributions, capability isolation,
  prompt checkpoints, persistence schemas, and CLI/control DTOs.

## Consequences

- **Positive:** Wardian gains a familiar editor/viewer without making editing
  the default or duplicating navigation.
- **Positive:** Agents can intentionally present work through a stable CLI
  instead of relying on brittle path detection or user promotion.
- **Positive:** User edits can reach an agent as reviewable intent without first
  overwriting the file.
- **Positive:** Prompt-scoped diffs become a shared backend capability that can
  expand into Explorer and broader change review.
- **Positive:** Live HTML/SVG artifacts remain useful while being denied Wardian
  privileges and network access.
- **Positive:** The authorization boundary matches user-granted agent roots and
  excludes hidden Wardian context roots.
- **Negative:** Monaco, PDF.js, content indexing, snapshot storage, and isolated
  live documents add bundle, memory, persistence, and security complexity.
- **Negative:** Maintaining an incremental index across several authorized roots
  introduces startup readiness and retention trade-offs.
- **Negative:** The first release intentionally provides no mobile/remote Files
  experience.
- **Negative:** A file and its artifact thread can coexist as two tabs because
  they represent different durable resources.

## Rejected Alternatives

### Separate Files and Artifacts surfaces

This duplicates renderers and makes users choose a product abstraction before
opening the same underlying content.

### Treat an artifact as metadata on a path-keyed file tab

This tangles mutable path identity with origin/version/review identity and
cannot represent two agents presenting the same file independently.

### Open every Explorer file as a permanent tab

This creates tab churn. One transient preview per group preserves rapid
inspection while edits, comments, double-click, or Pin make intent durable.

### Let the user decide when a file becomes an artifact

Manual promotion adds friction and loses origin/provenance. The producing agent
declares presentation through `wardian artifact present`; manual user opening
remains a file resource.

### Permit every OS-readable path

The provider process may have broader operating-system access than the user
intended to grant the agent. Wardian uses the primary workspace plus explicit
`include_directories` and fails closed outside them.

### Count `system_include_directories` as artifact roots

Those paths are Wardian-managed instruction and skill projections, not user
content grants. Including them would expose internal agent context as
publishable artifacts.

### Copy every artifact into Wardian storage

Copies break the live relationship with the backing file and create ambiguous
write-back semantics. Wardian snapshots versions while retaining the canonical
file as working truth.

### Make every file save an artifact version

Editor save patterns and atomic replacements would flood history with
implementation events. Versions are explicit presentation acts; working-state
changes are shown separately.

### Render HTML as source only

This defeats the purpose of HTML artifacts. Wardian instead runs them in a
capability-free isolated document host.

### Allow artifact network access

CDN and API access would improve compatibility but expand privacy,
reproducibility, and exfiltration risk. First-release artifacts are
self-contained and networkless.

### Attribute every prompt-scoped change to the agent

Other tools and users can edit the same roots. Wardian records a temporal
checkpoint and uses the honest label **Changes since prompt**.

### Lock files while users draft

This blocks normal agent/tool progress. Base hashes and three-way merging
preserve concurrent work without silent overwrite.

### Send each comment immediately

Immediate delivery fragments review context and creates noisy agent turns.
Wardian sends one structured batch when the user chooses Send to agent.

### Force blocking approval

Some artifacts require review while others are informational. Approval state is
visible and durable but does not implicitly pause the agent runtime.
