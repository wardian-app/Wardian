# Files Editor and Review Interaction

- **Status:** Approved
- **Date:** 2026-07-17
- **Primary issue:** [#392](https://github.com/wardian-app/Wardian/issues/392)
- **Related issues:** [#393](https://github.com/wardian-app/Wardian/issues/393), [#395](https://github.com/wardian-app/Wardian/issues/395), [#513](https://github.com/wardian-app/Wardian/issues/513)
- **Supersedes:** The Preview / Changes / Draft interaction model in
  `docs/specs/2026-07-16-files-artifact-review-surface.md` and
  `docs/specs/2026-07-17-files-source-toggle-and-path-display.md`

## Decision

Wardian Files uses a conventional editor model:

1. A file is either rendered or open in its source editor when both
   presentations exist. The compact Book/Pencil control switches between them.
2. Editing writes into an ordinary unsaved editor buffer. Saving is explicit.
3. Changes are annotations and a comparison lens over that buffer, not a
   mutually exclusive Files mode.
4. Artifacts use the same viewer and editor as ordinary files. Provenance,
   comments, approval, and **Send to agent** are additional review capabilities,
   not a separate Draft experience.

The visible Preview, Changes, and Draft tabs are removed. Wardian does not ask
users to classify the work they are doing before they can read, edit, or review
a file.

## Why this model

The three-mode model makes ordinary editing feel like a workflow product rather
than an editor. Preview and Draft are two presentations of the same working
content, while Changes is a comparison against another revision. Treating all
three as peers creates unclear transitions, duplicates state, and makes common
actions take more clicks.

Established agent tools keep these concepts separate:

- [Codex review](https://developers.openai.com/codex/app/review) presents review
  beside normal editing, supports line comments, and makes the comparison
  baseline explicit.
- [Antigravity Review Changes](https://antigravity.google/docs/review-changes-editor)
  opens a review pane inside the editor and anchors feedback to file diffs.
- [Claude Artifacts](https://support.claude.com/en/articles/9487310-what-are-artifacts-and-how-do-i-use-them)
  collects artifact feedback and sends it back to the agent rather than
  conflating feedback with a filesystem write.

Wardian follows those interaction patterns while retaining local files as the
source of truth and preserving the Workbench's multi-pane layout.

## User model

### Viewing and editing

- Markdown, HTML, and active SVG open rendered by default.
- Their Book/Pencil control shows the current presentation: Book while rendered,
  Pencil while editing source. Its tooltip and accessible label describe the
  action, such as **Edit source** or **View rendered**.
- Source code, plain text, configuration, and other source-only formats open
  directly in editable Monaco. They do not show a redundant presentation
  toggle.
- Images and PDFs remain read-only until a renderer advertises an editing
  capability. They do not show a misleading Pencil control.
- Switching presentation never reopens the file, creates another tab, or
  changes the active resource subscription.

Switching a rendered resource into its editor pins a transient preview. A
source-only file opened by Explorer single-click remains transient until the
first buffer mutation, comment, explicit Pin, or double-click.

### Explicit save

Monaco edits update an in-memory buffer and do not write on every keystroke.

- `Ctrl+S` on Windows/Linux and `Cmd+S` on macOS save the active file.
- The command palette and overflow menu expose **Save** and **Save As** where
  supported; the header does not carry a permanent text button.
- A dirty dot appears in the Workbench tab and beside the file breadcrumb.
- Saving performs a guarded atomic write against the content revision on which
  the buffer is based.
- A successful save advances the editor baseline and clears the dirty marker.
- A failed save preserves the exact buffer and reports a resource-local error.

Closing a dirty tab offers **Save**, **Don't Save**, and **Cancel**. Wardian
durably checkpoints a dirty buffer into backend-owned recovery state. Recovery
is not presented as a separate Draft mode. Choosing **Don't Save** deletes that
recovery state.

**Save As** is initially available only for ordinary file resources selected
through an exact native-picker grant. It creates and opens a new ordinary file
resource. It never retargets an artifact thread; artifact users may save a copy
without changing the artifact's backing-file identity.

If the backing file changes while the buffer is dirty, Wardian marks the buffer
stale and opens a three-way comparison before allowing an overwrite. It never
silently replaces either the user's buffer or the newer file.

### Operation snapshots

The implementation keeps three hashes distinct:

| Name | Meaning | Advances when |
|---|---|---|
| `buffer_base_hash` | Saved revision from which the current editor buffer was created or rebased | Save or accepted rebase succeeds |
| `disk_head_hash` | Latest stable authorized revision observed on disk | The resource watcher publishes a stable revision |
| `review_base_hash` | Immutable prompt checkpoint or presented version selected for historical comparison | The user selects another valid historical baseline |

Dirty means the buffer differs from `buffer_base_hash`. Stale means
`disk_head_hash` differs from `buffer_base_hash` while the buffer is dirty. The
two states may coexist. **Saved file** means the retained editor base even while
the disk head is newer. It is the one intentional exception to an immutable
review base: it aliases `buffer_base_hash`, so it advances and clears after
Save. No historical prompt or presented-version baseline advances on Save.

## Changes and comparison

Changes are defined by two independent concepts:

- **Dirty state:** current editor buffer compared with the retained
  `buffer_base_hash` revision.
- **Review changes:** current buffer or file compared with a selected historical
  baseline.

The dirty dot communicates only unsaved edits. Review decorations communicate
only the selected comparison. The UI must not use one indicator for both.

When an applicable review baseline exists, Monaco shows line-level decorations
in the normal editor:

- added and modified lines use themed gutter and background treatments;
- deletions use gutter markers and collapsible deleted-line zones;
- hovering or focusing a decoration identifies the active baseline;
- decorations update from the current buffer without requiring a save.

A compact `FileDiff` control shows the number of change regions and toggles the
full comparison lens. Its accessible description also reports added and deleted
line counts. The lens uses:

- side-by-side comparison when both sides meet the minimum readable width;
- unified comparison in narrow panes;
- a read-only baseline side and an editable current side for text;
- version side-by-side presentation for images and PDFs;
- renderer-local failure and unavailable states.

Opening the comparison lens does not change the Workbench tab identity and does
not create a nested tab strip. For rendered Markdown, HTML, and SVG, the lens
shows the source comparison without changing the underlying rendered/editor
presentation; closing it returns to the rendered document and its scroll
position.

`comparison_layout_preference` is persisted, while effective layout is computed
from the renderer and current content width:

| Content | Auto at 720 px or wider | Auto below 720 px |
|---|---|---|
| Text/source | Side by side | Unified |
| Image/PDF | Side by side | Vertically stacked |

A user-forced side-by-side preference is honored down to a 560 px hard minimum.
Below that minimum Wardian retains the preference but temporarily uses unified
text or stacked binary layout and explains the override in the layout control's
tooltip. For image and PDF renderers, a persisted `unified` preference maps to
stacked layout because there is no line-unified binary representation. Drawer
width is subtracted before applying these thresholds.

### Baselines

The active baseline is always named. Initial choices are:

- **Since last prompt** when a durable prompt checkpoint exists;
- **Presented version** for an artifact thread;
- **Saved file** for unsaved editor changes;
- **Previous presented version** when an artifact has version history.

The overflow menu exposes **Compare against** and the baseline choices. The
compact diff control's tooltip includes the active choice, for example
**7 changes since last prompt**. Git commits, branches, and staging scopes may
be added later without changing the comparison model.

Unless the user has selected another valid baseline, an artifact compares with
its selected presented version. An ordinary file compares with the most recent
prompt checkpoint in its agent context; without one, a dirty editor may compare
with its saved file, and a clean context-free file has no review baseline. A
manual baseline choice remains attached to the presentation until it becomes
invalid or the resource identity changes.

If a persisted historical baseline is deleted or becomes unauthorized, Wardian
clears it, closes the comparison lens, removes its decorations, and reports
**Comparison baseline unavailable**. Expired transport tickets are renewed and
do not invalidate an otherwise retained baseline.

Wardian never labels prompt-scoped differences as “agent changes.” A checkpoint
proves temporal scope, not authorship.

## Artifact collaboration

An artifact is a file resource with a versioned review relationship. It keeps
the same rendering, Monaco buffer, save shortcuts, dirty state, and close guard
as an ordinary file.

Artifact-only capabilities appear in the review lens or review drawer rather
than as additional top-level modes:

- origin agent and presentation provenance;
- presented-version history;
- line, range, spatial, and general comments;
- queued-feedback count;
- **Send to agent**;
- approval and resolution state.

An artifact remains identifiable with those panels closed. Its ordinary
file-type tab icon receives a small provenance badge, and a compact artifact
review icon beside the diff control opens the review drawer. The icon's tooltip
names the origin agent and queued-feedback count without adding a persistent
text label to the header.

Line and selection comments attach to the selected comparison version. Comments
remain queued until the user sends them, allowing one coherent review instead
of a stream of agent interruptions.

### Save and Send are independent

**Save** writes the current buffer to the backing file. **Send to agent** creates
an immutable review containing:

- the patch between the applicable review base and the current buffer;
- queued anchored comments;
- the general review note;
- exact file, version, checkpoint, and content-hash references.

Sending does not save. It may include an unsaved buffer, and the buffer remains
dirty afterward. Saving does not send feedback or imply approval. The user may
save, send, do both in either order, or do neither.

The submission patch is always based on the artifact version being reviewed,
not an arbitrary baseline selected for visual comparison. Changing **Compare
against** therefore cannot change what Wardian sends. If the reviewed version
is no longer a valid base, Send pauses for an explicit rebase or version choice.

At invocation, Send freezes a new `review_id`, the reviewed version and base
hash, current buffer hash and patch, general note, and IDs/content of all
currently queued comments. Comments added while delivery is in flight remain
queued for the next review. A retry reuses the same review ID and exact payload,
making delivery idempotent. The user may cancel a failed review and create a new
snapshot if they want later buffer changes included.

After a successful send, only the snapshotted comments become sent comments
associated with that review ID. If delivery fails, the immutable review and its
snapshotted queue remain intact and retryable.

Approval applies only to the selected immutable presented version. Sending a
review does not approve it, saving the backing file does not approve it, and a
new presented version returns the thread to awaiting review. Comment state
continues to follow the artifact lifecycle contract: agents may mark a comment
addressed, but only the user resolves it.

The first release exposes Approve only for the latest presented version. It is
enabled when that version is still authorized, the displayed content hash
matches it, and the version has no queued, failed, open, or
agent-marked-addressed-but-unresolved feedback. Approval durably records an
`approval_id`, artifact and version IDs, content hash, local approver identity,
and timestamp before notifying the origin agent. Notification is idempotent and
retryable with the same approval ID; failure shows **Approved, delivery
pending** and does not roll back local approval. Earlier version approvals
remain in history, but never imply approval of a newer version.

## Header and responsive layout

The Files header contains only high-frequency context and icon controls:

```text
[breadcrumb + dirty dot]          [Book/Pencil] [Diff count] [Artifact] [...]
```

The Artifact control appears only for artifact resources. Artifact review
actions may appear in a contextual strip inside the review drawer or lens. They
do not permanently crowd the file header.

The overflow trigger reuses the Workbench tab-strip treatment exactly: Lucide
`Ellipsis`, 17 px icon, 1.75 stroke width, and a 26 by 26 px target. Literal
ellipsis text and custom letter spacing are not used.

At narrow widths:

- the breadcrumb elides middle segments while preserving the basename;
- header icon controls remain reachable;
- the comparison lens switches to unified text or stacked image/PDF layout;
- the review drawer becomes an overlay;
- no toolbar introduces horizontal scrolling over the editor.

The content renderer may scroll horizontally when its content requires it.

## State and migration

Workbench persistence stores presentation and comparison intent, not file
contents or editor buffers:

```ts
type FilesSurfaceStateV2 = {
  resource_kind: "file" | "artifact";
  transient_preview: boolean;
  presentation: "rendered" | "editor";
  comparison_open: boolean;
  comparison_layout_preference: "auto" | "unified" | "side_by_side";
  comparison_baseline: FilesComparisonBaseline | null;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
};
```

Backend-owned stores retain unsaved recovery buffers, artifact comments,
reviews, immutable versions, and checkpoints. The Workbench document never
contains file bytes or executable artifact content.

The V1 migration is deterministic:

| V2 field | V1 source/default |
|---|---|
| `resource_kind` | Retain V1 value |
| `transient_preview` | Retain V1 value, except a legacy buffer pins it |
| `presentation` | `preview` uses the renderer default; `changes` and `draft` request `editor` |
| `comparison_open` | `true` only for `changes` with a resolvable baseline |
| `comparison_layout_preference` | `auto` |
| `comparison_baseline` | Prompt checkpoint when `optional_checkpoint_id` resolves; otherwise selected presented version for an artifact; otherwise `null` |
| `review_drawer_open` | Retain V1 value |
| `selected_version_id` | Retain V1 value |
| `optional_checkpoint_id` | Retain V1 value |

Before removing legacy Draft state, migration imports its bytes and base hash
into the backend recovery store. If no legacy bytes exist, `draft` migrates as a
clean editor. Migration is idempotent and records completion only after both
recovery and Workbench V2 state are durable.

Fallback order is explicit: an unsupported editor request becomes the
renderer default; an unsupported rendered request becomes editor when available
and otherwise the renderer default; an unavailable comparison baseline closes
comparison; an unsupported side-by-side request uses unified text or stacked
binary layout. Wardian persists the normalized presentation and baseline while
retaining the user's layout preference.

### Recovery after restart

Recovery is mandatory for every dirty editable Files buffer:

1. If the authorized disk head still matches `buffer_base_hash`, Wardian
   restores the dirty buffer and announces **Recovered unsaved changes**.
2. If the disk head changed, Wardian restores the buffer as stale and opens the
   three-way merge path before Save.
3. If authorization is unavailable, Wardian restores the recovery buffer
   read-only and offers **Restore access** and **Discard recovery**.
4. Restore access performs normal authorization again and opens a new verified
   handle. It never revives a revoked capability. The restored disk head is then
   compared with the preserved buffer base.
5. Explicit discard removes recovery only after the Workbench state no longer
   references it.

## Accessibility

- Every icon control has an action-oriented accessible label and a tooltip.
- Current presentation uses pressed-state semantics; the glyph reflects current
  state while the label describes the action.
- Diff decorations are not color-only: gutter symbols, accessible descriptions,
  and the review list expose the same information.
- Keyboard navigation can move between changes and comments without requiring
  pointer interaction.
- Save, discard, conflict, and send outcomes are announced through resource-local
  status regions.

## Error and concurrency behavior

- External modification while clean reloads the saved model while preserving
  view position where possible.
- External modification while dirty marks the buffer stale and offers exactly
  **Merge**, **Reload from disk**, and **Cancel**.
- Save uses the retained authorized handle and expected revision; path reopening
  does not bypass authorization.
- Authorization revocation makes the editor read-only, preserves recoverable
  unsaved content, and disables Save and Send until access is restored.
- Baseline expiry removes review decorations and explains why comparison is no
  longer available; it never changes the editor buffer.
- Renderer failure affects only the active resource presentation.

For a stale dirty buffer, **Merge** rebases the local edits onto the new disk
head and advances `buffer_base_hash` only after conflicts are resolved. The
result remains dirty until saved. **Reload from disk** discards the local buffer
and its recovery record, adopts `disk_head_hash`, and becomes clean. **Cancel**
keeps the stale buffer and recovery record unchanged. A clean external reload
adopts the new disk head as both editor and buffer base.

## Verification

### Frontend unit and browser tests

- Preview/Changes/Draft tabs are absent.
- Markdown Book/Pencil state, labels, keyboard operation, pinning, and source
  buffer behavior are correct.
- Source-only files open in editable Monaco without a redundant toggle.
- Dirty state, explicit save, failed-save preservation, and close decisions are
  lossless.
- Inline additions, modifications, deletions, counts, and baseline labels render
  independently of dirty state.
- Comparison selects side-by-side or unified layout from actual pane width.
- Artifact comments queue, send, retry, and remain independent from save.
- The header remains usable in a narrow Workbench pane and its ellipsis matches
  the tab-strip control.

### Native tests

- Guarded atomic save succeeds only for the retained authorized resource and
  expected revision.
- Concurrent file changes enter merge/conflict handling without data loss.
- Unsaved recovery survives a forced application restart and is deleted after
  explicit discard.
- Prompt and presented-version baselines resolve to immutable content hashes.
- Sending an unsaved artifact buffer produces the expected patch without
  writing the backing file.
- Review delivery failure remains durable and retryable.

### Repository verification

- `npm run lint`
- `npm run test`
- `npm run build`
- `npm run test:e2e`
- `cargo clippy --workspace -- -D warnings` in `src-tauri`
- `cargo test --workspace -- --test-threads=1` in `src-tauri`
- `cargo check --workspace` in `src-tauri`
- feature-specific screenshots for rendered, edited, inline-change, comparison,
  and artifact-feedback states

## Consequences

- Ordinary files feel like familiar editor documents instead of workflow state
  machines.
- Review remains available without displacing editing or multiplying tabs.
- Artifact feedback gains explicit provenance and delivery without coupling it
  to disk writes.
- Unsaved recovery retains the safety benefit of the prior Draft store without
  exposing Draft as product vocabulary.
- Dirty state and historical comparison require separate baselines and visual
  indicators, increasing internal state precision while reducing user-facing
  complexity.

## Rejected alternatives

### Keep Preview, Changes, and Draft tabs

The labels expose implementation concepts and make ordinary editing require a
mode transition. Changes also does not belong at the same conceptual level as
viewing and editing.

### Autosave ordinary and artifact files

Autosave obscures when agent-produced work is mutated and weakens the user's
control over local files. Explicit save is consistent with Wardian's
inspectable, filesystem-first model.

### Make Send to agent save first

This prevents users from proposing a patch without mutating the working file
and conflates review intent with filesystem state.

### Put all artifact actions in the file header

Persistent action bars consume scarce pane width and recreate the button-heavy
navigation problem. Review actions belong with review context.

### Attribute baseline differences to the agent

Other agents, tools, and the user can modify the same authorized roots. Wardian
describes the exact temporal or version baseline instead of claiming authorship.
