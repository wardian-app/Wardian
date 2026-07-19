# Files Source Toggle and Path Display

## Status

Approved design for the Files Workbench surface.

## Purpose

Rendered previews must not hide the source material from the user. Text-backed
renderers may offer a source presentation within Preview, while the existing
Preview, Changes, and Draft modes retain their separate lifecycle meanings.
Filesystem paths must also be presented in the same human-readable form used
by Explorer without weakening the backend's canonical-path authority.

## Mode model

- **Preview** displays the current file revision. A renderer may offer more
  than one presentation of that revision, such as rendered Markdown and raw
  Markdown source.
- **Changes** compares the file with an applicable baseline. It is not a raw
  source view.
- **Draft** is Wardian's editable working copy before changes are applied to
  the underlying file.

The source toggle is therefore a Preview presentation control rather than a
fourth Files mode.

## Source presentation

The Files header places one compact icon control beside the existing file
actions when the selected renderer supports both rendered and source
presentations.

- Rendered Markdown shows a reading/preview icon with the accessible label and
  tooltip **View source**.
- Markdown source shows a pencil/edit icon with the accessible label and
  tooltip **View rendered**.
- The glyph reflects the active presentation, while the accessible label and
  tooltip describe the action the control will perform.
- The control exposes its current state through standard pressed-state
  semantics and remains keyboard operable.
- Plain text and source-code files remain source-only and do not show a
  redundant toggle.
- Images, PDFs, unsupported content, and unavailable resources do not show a
  source toggle.

The presentation choice is local to the mounted Files tab. It survives normal
tab switching while that presentation remains mounted and defaults to rendered
after restoration or when the resource changes.

## Renderer architecture

Source support is declared by the renderer definition rather than by checking
for Markdown inside the Files shell. A renderer may provide an optional source
renderer factory. The Files preview host resolves the normal renderer and, when
available, its source renderer through the same registry boundary.

Markdown uses the existing Monaco text renderer for source presentation. Both
presentations read the same backend-validated snapshot and revision through the
existing file-resource client. Switching presentation does not reopen the file,
create another Workbench surface, or change the subscription.

Renderer load failures remain resource-local. Resetting the renderer resets the
active presentation instance without changing the selected file revision.

## Display paths

Wardian keeps canonical paths unchanged for authorization, resource identity,
IPC, Open With, Reveal, and local-link resolution. A separate formatter is used
only for visible text, titles, tooltips, and breadcrumb segments.

The formatter preserves case and native separators. On Windows it removes the
extended-length prefix:

- Windows-specific: `\\?\C:\workspace\notes.md` is displayed as
  `C:\workspace\notes.md`.
- Windows-specific: `\\?\UNC\server\share\notes.md` is displayed as
  `\\server\share\notes.md`.

Ordinary Windows drive paths, UNC paths, and non-Windows paths otherwise remain
unchanged. Comparison normalization remains separate because display formatting
must not lowercase paths or affect equality checks.

## State and data flow

1. The backend returns the canonical descriptor and validated revision.
2. The renderer registry selects the normal preview and reports whether a
   source renderer exists.
3. The Files preview host owns `rendered` or `source` state for the mounted tab.
4. The icon control changes only that local presentation state.
5. The mode bar formats the canonical path for display while retaining the
   original path for every action callback.

## Verification

Unit and browser coverage must prove:

- Markdown switches from rendered output to Monaco source and back without a
  new resource subscription.
- The toggle has correct accessible labels, tooltip, pressed state, focus, and
  keyboard activation.
- Plain text, images, PDFs, unavailable content, and unsupported content do not
  expose a misleading toggle.
- Changing resource identity returns the presentation to rendered.
- Windows drive and UNC extended prefixes are removed only from displayed
  paths, with case and separators preserved.
- Open With, Reveal, resource keys, Markdown link resolution, and backend calls
  continue receiving the original canonical path.
- The control remains reachable in narrow panes and does not displace the
  Preview, Changes, or Draft tabs.
