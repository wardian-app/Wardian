# cmux Multi-Surface GUI References

This document maps cmux's multi-surface workspace model to design patterns
relevant to Wardian's agent command center, split/tab surfaces, and
multi-agent coding UI.

This is not an endorsement, affiliation claim, product evaluation, or
competitive teardown. The notes below describe public architecture and design
pressure only.

Last reviewed: 2026-06-07.

Source basis: source observations are based on cmux commit `8c130f8`.

## Context

cmux feels smooth for multi-agent coding because terminals, browser panes, file
previews, markdown, and agent-oriented panels all share one workspace, pane, and
surface model. The browser is not a secondary preview area, and files are not a
separate viewer mode. They are peer panels inside the same split/tab graph as
terminals.

## Core Pattern

cmux models visible work as a graph:

```text
window -> workspace -> pane -> surface/panel
```

The important product-design consequence is that every tool surface can use the
same operations: split, tab, focus, move, close, flash, automate, and restore.
The user does not have to remember whether a tool is a "terminal thing", a
"browser thing", or a "file thing".

## Feature-By-Feature Breakdown

### 1. Unified panel protocol

cmux defines a single `PanelType` enum for `terminal`, `browser`, `markdown`,
`filePreview`, `rightSidebarTool`, `agentSession`, `project`, and
`extensionBrowser`. All panel implementations conform to the same `Panel`
protocol, including identity, display title, icon, dirty state, focus,
unfocus, close, flash, and focus restoration.

Implementation effect:

- New surface types can join the split/tab system without inventing a new
  layout stack.
- Focus and attention behavior is defined once at the panel contract boundary.
- The UI can reason about terminals and non-terminals uniformly.

Sources:

- `Sources/Panels/Panel.swift`: `PanelType` and `Panel` protocol.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/Panel.swift#L6-L14>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/Panel.swift#L270-L319>

### 2. One split/tab renderer

`WorkspaceContentView` renders a `BonsplitView`. For each tab, cmux resolves the
tab id back to a workspace panel, calculates focus, visibility, split state, and
notification ring state, then passes the panel into `PanelContentView`.

`PanelContentView` switches on `panel.panelType` and renders the correct
surface-specific view. The layout stack does not know whether a tab is terminal,
browser, file preview, or markdown beyond the panel type dispatch.

Implementation effect:

- Browser and file previews inherit the same pane placement, tab selection, and
  split behavior as terminals.
- The app avoids a fragmented "terminal tabs plus auxiliary previews" mental
  model.
- Inactive workspaces can stay mounted for state preservation while interaction
  is disabled.

Sources:

- `Sources/WorkspaceContentView.swift`: `BonsplitView` and panel lookup.
- `Sources/Panels/PanelContentView.swift`: panel-type dispatch.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/WorkspaceContentView.swift#L202-L240>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/PanelContentView.swift#L32-L145>

### 3. Terminal fidelity as a native surface

Terminal tabs are `TerminalPanel` instances wrapping `TerminalSurface`.
Rendering uses `GhosttyTerminalView`, backed by Ghostty/libghostty rather than
a generic web terminal. `TerminalPanelView` keeps the representable identity
stable with `.id(panel.id)` so split/tab structure changes do not transiently
tear down the hosted terminal view.

Implementation effect:

- Terminals preserve scrollback, focus, rendering, and input fidelity during
  split and tab operations.
- The terminal remains the primary primitive, not an embedded approximation.
- Reparenting and close/reopen paths are treated as lifecycle problems, not just
  React/SwiftUI layout changes.

Sources:

- `Sources/Panels/TerminalPanel.swift`: terminal panel wraps `TerminalSurface`.
- `Sources/Panels/TerminalPanelView.swift`: `GhosttyTerminalView` and stable id.
- `README.md`: Swift/AppKit, libghostty, Ghostty config compatibility.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/TerminalPanel.swift#L31-L45>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/TerminalPanelView.swift#L63-L83>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/README.md#L121-L123>

### 4. Browser as a first-class surface

Browser tabs are `BrowserPanel` instances implementing the same `Panel`
protocol. Internally, a browser panel owns a `WKWebView`, profile/history state,
website data store, devtools behavior, user-agent configuration, and remote
proxy state.

Browser creation is available through the same command surface as other
operations. The v2 dispatcher exposes `browser.open_split`, `browser.navigate`,
`browser.back`, `browser.forward`, `browser.reload`, and other browser methods.

Implementation effect:

- A browser can open beside the current terminal as a peer split, not as a
  separate application or passive preview.
- Authenticated browser state persists through profile and website data store
  management.
- Browser automation and manual browsing operate on the same browser surface.

Sources:

- `Sources/Panels/BrowserPanel.swift`: `BrowserPanel`, `WKWebView`, website
  data store, proxy state.
- `Sources/TerminalController.swift`: browser v2 socket commands.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/BrowserPanel.swift#L3007-L3117>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/BrowserPanel.swift#L4093-L4124>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/TerminalController.swift#L2001-L2010>

### 5. Browser automation in the same handle model

cmux exposes browser automation commands including open, navigate, snapshot,
eval, wait, click, fill, press, and screenshot. The README describes this as a
scriptable API ported from `agent-browser`, letting agents inspect page state
and interact with development servers directly.

Implementation effect:

- Browser automation targets the same browser pane the user can see.
- Agents can run browser checks without leaving the workspace.
- The browser becomes a coding surface, not just a preview.

Sources:

- `README.md`: in-app browser and automation description.
- `docs/cli-contract.md`: browser command family.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/README.md#L127-L129>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/docs/cli-contract.md#L243-L255>

### 6. File and markdown surfaces

File previews are also panels. `FilePreviewPanel` implements `Panel`, owns the
file path, title, icon, dirty state, preview mode, native view sessions, text
loading, and save behavior.

The `cmux open` command routes:

- files to `file.open`
- URLs to `browser.open_split`
- directories to `workspace.create`

Multiple files open as tabs in the same target pane.

Implementation effect:

- File inspection sits in the same workspace as the agent session.
- A terminal command can open a file preview next to itself without leaving the
  layout.
- Markdown, text, PDF, media, and image previews can share panel lifecycle.

Sources:

- `Sources/Panels/FilePreviewPanel.swift`: file preview panel state.
- `CLI/cmux_open.swift`: `open` routing and command usage.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/FilePreviewPanel.swift#L971-L1024>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/CLI/cmux_open.swift#L696-L739>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/CLI/cmux_open.swift#L6032-L6043>

### 7. Handle-based socket and CLI API

cmux v2 is explicitly handle-based: `window_id`, `workspace_id`, `pane_id`, and
`surface_id`. The socket API exposes workspaces, surfaces, panes, input,
notifications, and browser commands over the same protocol.

Implementation effect:

- UI commands, CLI commands, agent hooks, and remote relays all address the same
  runtime graph.
- Surfaces can be moved, focused, split, automated, and inspected by id.
- The API shape reinforces the product mental model.

Sources:

- `docs/v2-api-migration.md`: handle-based protocol and implemented methods.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/docs/v2-api-migration.md#L7-L23>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/docs/v2-api-migration.md#L78-L114>

### 8. Context-aware terminal-launched commands

Inside cmux terminals, environment variables provide the current surface
context:

- `CMUX_WORKSPACE_ID`
- `CMUX_SURFACE_ID`
- `CMUX_TAB_ID`

`cmux open` defaults to these values when explicit handles are not provided.
That lets a terminal command open a file or browser split in the user's current
workspace and next to the current surface.

Implementation effect:

- Launch friction is low: agents and users do not need to specify placement for
  common operations.
- Automation naturally stays local to the active work context.
- Terminal-originated commands feel like extensions of the current pane.

Sources:

- `docs/cli-contract.md`: environment variable contract.
- `CLI/cmux_open.swift`: defaults from environment.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/docs/cli-contract.md#L48-L55>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/CLI/cmux_open.swift#L696-L700>

### 9. Sidebar metadata is a projection of runtime state

cmux's sidebar does more than list tabs. The README says it shows branch,
linked PR status/number, working directory, listening ports, and latest
notification text. The data-driven sidebar plan documents richer state:
workspace id/ref, title, pinned state, remote state, current directory, unread
count, latest notification text, pull request URLs, panel directories, and git
branches.

Implementation effect:

- Multi-agent state is scannable without opening each tab.
- Sidebar rows communicate both place and status.
- The navigation model is work-centered rather than process-centered.

Sources:

- `README.md`: sidebar metadata summary.
- `docs/data-driven-sidebar-plan.md`: runtime state payloads.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/README.md#L60-L64>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/docs/data-driven-sidebar-plan.md#L34-L40>

### 10. Attention and notification state

cmux has workspace/panel attention primitives for notification rings and flash
decisions. Notifications can attach to workspace and surface context. The README
describes terminal OSC notifications and `cmux notify` integration; when an
agent waits, its pane gets a ring and the sidebar lights up.

Implementation effect:

- Attention is spatial: "this pane in this workspace needs me."
- The user can triage many agents without reading every terminal.
- Notification state is both visual and addressable through commands.

Sources:

- `Sources/Panels/Panel.swift`: attention state and flash coordinator.
- `README.md`: notification system and unread jump behavior.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/Sources/Panels/Panel.swift#L82-L169>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/README.md#L125-L129>

### 11. Remote workspace and browser relay

For SSH workflows, cmux creates a remote workspace. Browser panes route through
the remote network so remote localhost works. Remote CLI commands can control
the local cmux browser through an authenticated relay. Browser commands that
target an existing surface default to `CMUX_SURFACE_ID`; browser `open`
defaults to `CMUX_WORKSPACE_ID`.

Implementation effect:

- Remote terminal and local browser remain part of one workspace.
- Browser automation can be initiated from inside the remote shell.
- The relay preserves the same handle-based model across machine boundaries.

Sources:

- `README.md`: SSH workspace and remote browser routing.
- `daemon/remote/README.md`: relay and browser command defaults.
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/README.md#L69-L70>
- <https://github.com/manaflow-ai/cmux/blob/8c130f8/daemon/remote/README.md#L143-L165>

## Wardian Implications

Wardian should not treat cmux's browser integration as a browser feature alone.
The reusable pattern is a first-class surface graph with common lifecycle and
automation semantics.

Recommended direction:

1. Define a Wardian surface contract that covers id, type, title, icon,
   lifecycle, focus, attention, layout placement, and automation handles.
2. Model terminals, browser previews, files, logs, markdown, agent feeds,
   reviews, and future tools as surfaces inside workspaces and panes.
3. Keep terminal runtime authority in the backend, but expose a handle graph
   that frontend UI, CLI, and agents can all address.
4. Add context environment variables for terminal-launched commands so agents
   can open browser/file surfaces beside their current session without manual
   placement.
5. Make the sidebar a projection of workspace and surface state, not a static
   navigation list.
6. Treat attention as workspace/surface state, with unread counts, latest
   notification text, status color, and visible pane indicators.
7. Preserve heavy surface state during tab and split operations rather than
   recreating terminals, browsers, or file previews on each layout change.

## Design Principle

The clean part of cmux is not vertical tabs by themselves. It is that vertical
tabs, horizontal splits, terminal fidelity, browser automation, file previews,
notifications, CLI commands, and remote relay all point at the same object
model. That is the standard Wardian should aim for.
