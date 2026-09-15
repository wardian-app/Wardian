# Preserve terminal hyperlinks in canonical snapshots

Issue: #1336

## Problem

Provider TUIs can render a labelled link using OSC 8: the visible text and its
destination are separate data. Wardian forwards these bytes during live output,
but its current vt100 parser ignores OSC 8. Canonical snapshots retain the label
and styling while losing the destination. Ordinary URL detection cannot recover
a destination from a label such as `PR #1324`.

The desktop renderer already supports HTTP(S) OSC 8 activation. Trusted file
hyperlinks need the existing validated file-opening path. Browser-facing remote
views must use browser navigation and must not interpret host files as files on
the viewing machine. Chat Markdown follows a separate rendering path and needs
separate verification.

## Decision

Keep the backend terminal screen as the canonical owner. Preserve bounded OSC 8
metadata in the parser's cells and active state so ordinary grid operations move
the metadata with the text. Use a narrow vendored vt100 correction through the
workspace's existing dependency-patch mechanism. Upstream vt100 0.16.2 and the
inspected upstream main implementation do not preserve this metadata.

Do not infer destinations from labels, build a second cursor/scroll state
tracker, or rely on a frontend-only cache. These approaches cannot reliably
restore a terminal that has not previously been displayed.

## Required behavior

- Snapshot and formatted-scrollback serialization retain hyperlink targets.
- Erase, overwrite, resize, scrolling, alternate buffers, and reset preserve the
  association between each displayed cell and its intended target.
- Serialization closes links at boundaries and restores the active link state
  needed by subsequent output. Adjacent unlinked text remains unlinked.
- Target length and retained metadata have explicit bounds. Rejected metadata
  must not prevent ordinary text from rendering or inject terminal controls.
- A shared link allocation serves its cells; reference release reclaims retained
  metadata. No ever-growing destination registry is introduced.
- Desktop file targets use existing validation and opening preferences. Unknown
  schemes are not forwarded to an unrestricted external opener.
- Remote web links use the viewing browser. Host-local file opening is not
  implicitly enabled by this change.

## Validation boundary

Parser and broker tests establish preservation, isolation, and memory bounds.
Browser tests exercise actual pointer activation after snapshot restoration and
remount, with plain URLs and adjacent unlinked text as controls. Provider-specific
filters are covered for all entries in the current provider list.

Seeded browser tests do not establish native PTY or real-provider acceptance.
Native evidence must identify the built artifact, isolated session, and emitted
target. Provider evidence must distinguish an emitted OSC 8 target from a visible
URL fallback or a label with no destination. Existing text-only snapshots cannot
recover metadata already discarded by an older parser.

## Maintenance

Retain upstream provenance and licensing with the vendored dependency. Review
only the local delta when updating it. Remove the patch when an upstream version
supports the required preservation and passes Wardian's regression tests.
