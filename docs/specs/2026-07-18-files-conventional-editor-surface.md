# Files Conventional Editor Surface

- **Status:** Implemented
- **Date:** 2026-07-18
- **Primary issue:** [#392](https://github.com/wardian-app/Wardian/issues/392)
- **Parent design:** `2026-07-17-files-editor-review-interaction.md`

## Decision

The Files surface implements ordinary editor semantics instead of visible
Preview, Changes, and Draft modes. A resource advertises explicit rendered and
editor presentations. Markdown provides both and starts rendered; validated
text, HTML, and SVG provide an editor only; images and PDFs provide a renderer
only. HTML and SVG remain inert source and are never injected into Wardian's DOM.

The Book/Pencil control reflects the current presentation while its accessible
label names the action. The overflow button uses the same compact geometry as
Workbench chrome. Paths use Explorer-safe display spelling, including removal
of Windows extended-length prefixes, and middle-elide before pushing controls
out of narrow panes.

## Editor ownership

One canonical backend `resource_id` maps to one shared `FileEditorController`
and one Monaco model URI. Multiple panes create independent editor views over
that model. Presentation switches and file revision changes do not recreate the
model. The controller registry is the lifetime authority, preserving undo state
until the final view and durable recovery hold are released.

Rendered Markdown reads the controller's immutable buffer snapshot when dirty,
so switching presentations shows current edits without an extra native read.
Renderer failures are scoped to resource plus presentation and can be retried
without reopening the authorized resource.

## Save contract

Monaco mutation events update the shared working buffer and pin transient tabs.
Ctrl/Cmd+S and **File actions > Save** call the controller's revision-and-hash
guarded native save. Successful saves advance the baseline and clear dirty
state; stale and failed saves retain the exact buffer.

**Save As** is a transaction:

1. obtain a one-shot exact native picker grant;
2. write the current working buffer through that grant;
3. only after native success, open the returned canonical path as an ordinary
   file surface.

Save As never changes the current controller identity, clears its dirty buffer,
or retargets an artifact.

## Presentation badges

Dirty, attention, recovery, and future surface state are generic registry
badges. `WorkbenchHost` observes presentation invalidation and passes badge data
through both Dockview and the safe layout adapter. `WorkbenchTab` renders themed
dots and never changes the title with an asterisk. Files also shows the dirty
dot beside its breadcrumb.

## Verification boundary

Unit tests cover renderer capability resolution, stable model ownership,
controller/model synchronization, keyboard save routing, presentation switching,
Save As ordering, and both tab rendering paths. Browser E2E uses the real Monaco
native EditContext and a stateful IPC mock that enforces revision/hash CAS and
one-shot Save As grants.
