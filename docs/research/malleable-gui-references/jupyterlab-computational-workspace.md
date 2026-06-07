# JupyterLab Computational Workspace References

This document maps JupyterLab's computational workspace model to design
patterns relevant to Wardian's agent command center, restorable surface
layouts, document/session state, and extension-hosted tools.

This is not an endorsement, affiliation claim, product evaluation, or
competitive teardown. The notes below describe public architecture and design
pressure only.

Last reviewed: 2026-06-07.

Source basis: observations are based on `jupyterlab/jupyterlab` commit
`79150668bc1` and public JupyterLab user and extension documentation available
on 2026-06-07.

## Context

JupyterLab centers on notebooks and computational documents. Its main work area
can host notebooks, text files, terminals, consoles, inspectors, rendered
outputs, and extension widgets, but the mental model remains "interactive
computing session" rather than generic desktop layout.

For Wardian, JupyterLab is useful because it shows a mature browser-based
application where many independently authored tools become movable,
restorable, command-addressable widgets inside one shell. It also shows the
limits of that model: layout is very flexible, but terminal fidelity and
multi-process orchestration are not the product's center of gravity in the way
they are for Wardian.

## Core Pattern

JupyterLab's visible work is organized through a shell with named areas and
restorable widgets:

```text
JupyterFrontEnd
  -> LabShell
    -> top / menu / left / main / down / right / bottom / header areas
      -> Lumino widgets
        -> document widgets, notebook panels, terminals, side panels, extension widgets
          -> document context, kernel/session context, widget tracker, state DB keys
```

The key product-design consequence is that notebooks, terminals, file editors,
debugger panels, sidebars, and extension tools can be added to known shell
areas while still participating in commands, focus tracking, restoration, and
workspace save/load.

## Feature-By-Feature Breakdown

### 1. The shell has named placement areas

JupyterLab's extension docs describe the front-end shell as the API used to add
and interact with application content. The shell exposes areas for `top`,
`menu`, `left`, `right`, `main`, `down`, `bottom`, and `header`.

In source, `LabShell` constructs the frame from Lumino panels: a header panel,
menu handler, top handler, bottom panel, split panels, an optimized dock panel
for the main area, a tab panel for the down area, and side bar handlers.

Implementation effect:

- Extensions do not directly mutate the page layout; they add Lumino widgets
  to named shell areas.
- The product can keep a stable application skeleton while allowing many
  independently authored tools.
- Wardian can use the same idea for agent surfaces: a fixed control/roster
  frame plus a flexible main surface map and lower information area.

Sources:

- JupyterLab Common Extension Points: Jupyter Front-End Shell.
- `packages/application/src/shell.ts`: `LabShell` constructor.
- <https://jupyterlab.readthedocs.io/en/latest/extension/extension_points.html#jupyter-front-end-shell>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/shell.ts#L368-L410>

### 2. The main area is a docked tab/split workspace

The user docs describe the main work area as a place where documents and other
activities can be arranged into tab panels that can be resized or subdivided.
Dragging a tab to a panel center moves it into that tab panel; dragging to an
edge subdivides the panel.

In source, `LabShell` creates an `OptimizedDockPanelSvg` for the main area.
The shell exposes a `mode` property backed by the dock panel. Switching between
single-document and multiple-document mode caches/restores the dock layout
using `dock.saveLayout()` and `dock.restoreLayout()`.

Implementation effect:

- Notebooks, editors, terminals, and other main activities share the same tab
  and split mechanics.
- Focused single-document mode does not destroy the user's multi-document
  workspace; it caches the layout and restores it later.
- Wardian can treat "solo focus" as a temporary projection of a layout rather
  than a destructive layout mutation.

Sources:

- JupyterLab Interface: Main Work Area and Simple Interface mode.
- `packages/application/src/shell.ts`: dock panel creation, mode switching,
  cached layout restore.
- <https://jupyterlab.readthedocs.io/en/latest/user/interface.html#main-work-area>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/shell.ts#L400-L410>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/shell.ts#L677-L760>

### 3. Workspaces persist UI state, not just open files

JupyterLab sessions reside in workspaces. The user docs define a workspace as
layout and UI state: open files, notebooks, sidebars, panel open/closed state,
and area/tab layout. Workspaces can be managed through GUI commands, CLI
import/export, and URL schema. The workspace file format is JSON with `data`
and `metadata`; `data` maps to `IStateDB`, and plugins registered with
`ILayoutRestorer` store keys in that state.

Implementation effect:

- Workspace identity is concrete and portable enough to export/import.
- Layout restoration is coordinated through state keys rather than browser DOM
  snapshots.
- Wardian should make Habitat layouts serializable, named, resettable, and
  inspectable on disk.

Sources:

- JupyterLab Workspaces docs.
- `packages/application/src/layoutrestorer.ts`: layout fetch/dehydrate format.
- <https://jupyterlab.readthedocs.io/en/latest/user/workspaces.html>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/layoutrestorer.ts#L159-L222>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/layoutrestorer.ts#L330-L357>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/layoutrestorer.ts#L603-L662>

### 4. Restoration is plugin-aware and tracker-based

`LayoutRestorer` has a deliberately staged lifecycle. Plugins initialize,
register widget trackers or named widgets with the restorer, restore their own
widget instances, then the restorer hands a layout object to the shell. The
source comment is explicit that plugin restoration is accomplished by executing
commands and those commands must resolve only after the widget has been created
and added to the plugin's tracker.

Implementation effect:

- The layout restorer does not need to know how to recreate every widget type.
- Each extension owns its own restoration logic but participates in one shell
  layout restore.
- Wardian can use the same pattern for surface providers: each surface type
  registers how to restore itself, while the Habitat shell restores placement.

Sources:

- `packages/application/src/layoutrestorer.ts`: restoration lifecycle and
  `ILayoutRestorer` contract.
- `packages/apputils/src/widgettracker.ts`: widget tracker current-widget and
  restoration tracking.
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/layoutrestorer.ts#L32-L110>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/apputils/src/widgettracker.ts#L18-L80>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/apputils/src/widgettracker.ts#L116-L180>

### 5. Document widgets separate file context from UI placement

The document manager's source comment states that it maintains a context for
each open path/model type and a list of widgets for each context. When opening
or creating a document, it selects a widget factory, model factory, kernel
preference, document context, and widget, then opens the widget through the
opener.

`DocumentWidget` extends `MainAreaWidget` and owns a document context. It
listens to path changes, dirty-state changes, and context readiness so its
title and UI state stay synchronized with the file model.

Implementation effect:

- The file/session model is not embedded in the layout manager.
- Multiple widgets can share or derive from the same document context.
- Wardian should keep agent/session/task context separate from surface
  placement so the same agent can be viewed through terminal, transcript, diff,
  metrics, or browser surfaces.

Sources:

- `packages/docmanager/src/manager.ts`: document manager context ownership and
  `_createOrOpenDocument`.
- `packages/docregistry/src/default.ts`: `DocumentWidget`.
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/docmanager/src/manager.ts#L38-L46>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/docmanager/src/manager.ts#L657-L724>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/docregistry/src/default.ts#L549-L668>

### 6. Widget factories are the surface-type registry

`ABCWidgetFactory` captures metadata about a document widget type: name, label,
file types, model name, kernel preference flags, toolbar factory, and content
provider. Its `createNew()` method creates the widget through the subclass,
adds a toolbar, emits `widgetCreated`, and returns the widget.

Notebook support follows the same factory pattern. `NotebookWidgetFactory`
extends `ABCWidgetFactory`, owns notebook-specific services such as
`rendermime`, content factory, mime type service, and editor/notebook config,
then creates a `NotebookPanel`.

Implementation effect:

- New document/surface types join the system by registering factories, not by
  forking the shell.
- Toolbar behavior and creation signals are standardized.
- Wardian should define a `SurfaceFactory` or `SurfaceType` contract with
  title, icon, context binding, toolbar/region definitions, restore data, and
  commands.

Sources:

- `packages/docregistry/src/default.ts`: `ABCWidgetFactory`.
- `packages/notebook/src/widgetfactory.ts`: `NotebookWidgetFactory`.
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/docregistry/src/default.ts#L317-L345>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/docregistry/src/default.ts#L460-L505>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/notebook/src/widgetfactory.ts#L18-L105>

### 7. Notebooks bind UI, document model, and kernel/session state

`NotebookPanel` extends `DocumentWidget<Notebook, INotebookModel>`. Its
constructor assigns the notebook model from the document context, connects to
kernel changes, session status changes, and save-state events, and exposes
`sessionContext` and `model` accessors.

Implementation effect:

- The notebook surface is both a document view and a live runtime session view.
- Kernel/session state updates are attached to the panel context rather than
  treated as global application status.
- Wardian can mirror this for agent sessions: an agent surface should bind file
  state, process/session state, and UI status through one context object.

Sources:

- `packages/notebook/src/panel.ts`: `NotebookPanel`.
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/notebook/src/panel.ts#L35-L125>

### 8. Terminals are normal widgets, but with runtime fidelity concerns

JupyterLab terminals provide system shell access on the machine where the
Jupyter server runs. In source, `Terminal` is a Lumino `Widget` that wraps a
terminal session connection and creates an xterm.js terminal. It buffers output
while xterm initializes, connects to session messages, can dispose on session
exit, and resizes the terminal session based on xterm rows/columns.

Implementation effect:

- Terminals can live in the same shell areas as notebooks and editors.
- Terminal runtime concerns still require special handling: stream buffering,
  focus escape instructions, resize propagation, theme handling, and session
  lifecycle.
- Wardian should borrow the shell integration pattern, but keep stronger PTY
  fidelity and lifecycle guarantees than a generic widget registry provides.

Sources:

- JupyterLab Terminals docs.
- `packages/terminal/src/widget.ts`: terminal widget and resize.
- `packages/terminal/src/tokens.ts`: terminal widget tracker token.
- <https://jupyterlab.readthedocs.io/en/latest/user/terminal.html>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/terminal/src/widget.ts#L51-L130>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/terminal/src/widget.ts#L525-L545>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/terminal/src/tokens.ts#L13-L37>

### 9. Sidebars are persistent tool areas, not just navigation

The user docs describe left sidebar tabs such as file browser, open tabs,
running kernels/terminals, table of contents, and extension manager. The right
sidebar hosts property inspector and debugger. Tabs can move between left
sidebar, right sidebar, main work area, and down area from context menus.

The extension docs position left/right sidebars as areas for persistent UI
elements and expect the top-level object added there to be a Lumino `Widget`.
The interface customization docs also describe default widget placement by type
and user moves that are saved into workspace layout.

Implementation effect:

- Sidebars can host real tools, not only links.
- Movement between sidebar/main/down areas is constrained and policy-driven.
- Wardian's left control rail and right roster should stay stable by default,
  but selected panels could become movable surfaces when that improves a task.

Sources:

- JupyterLab Interface: sidebars.
- JupyterLab Interface Customization.
- JupyterLab Common Extension Points: Left/Right Areas.
- <https://jupyterlab.readthedocs.io/en/latest/user/interface.html#left-and-right-sidebar>
- <https://jupyterlab.readthedocs.io/en/latest/user/interface_customization.html#layout>
- <https://jupyterlab.readthedocs.io/en/latest/extension/extension_points.html#left-right-areas>

### 10. Commands are the integration currency

JupyterLab extensions add launcher items, menus, keyboard shortcuts, and shell
widgets through commands. The shell docs show adding content by executing a
command, receiving a widget, and adding it to an area. The layout restorer
notes also say plugin data restoration happens by executing commands.

Implementation effect:

- User affordances, plugin restoration, launcher entries, and shell insertion
  can share one command system.
- Widgets can be created lazily through commands rather than eagerly embedded
  in a global component tree.
- Wardian should keep a command registry as the bridge between UI operations,
  CLI actions, agent-driven actions, and restore-time surface creation.

Sources:

- JupyterLab Common Extension Points: launcher, shell, plugin state.
- `packages/application/src/layoutrestorer.ts`: command-based restoration
  lifecycle.
- <https://jupyterlab.readthedocs.io/en/latest/extension/extension_points.html#launcher>
- <https://jupyterlab.readthedocs.io/en/latest/extension/extension_points.html#jupyter-front-end-shell>
- <https://github.com/jupyterlab/jupyterlab/blob/79150668bc105ae59169c3b9af2baeff409254f4/packages/application/src/layoutrestorer.ts#L65-L109>

## Why JupyterLab Feels Smooth

JupyterLab feels smooth because it keeps several boundaries clear:

- Shell areas are named and stable.
- The main area is a real dock/tab/split workspace.
- Widget placement is independent from document/session context.
- Surface types are registered through factories and commands.
- Extensions restore themselves through trackers, while the shell restores
  placement.
- Workspaces are serializable and can be managed through GUI, CLI, and URL.
- Sidebars and down-area panels are tool surfaces, not just decorative chrome.

The result is a browser application that behaves like a desktop workbench
without giving every component unconstrained freedom.

## Wardian Implications

JupyterLab suggests a Wardian model that separates shell, layout, surface, and
session context:

```text
WardianShell
  -> named areas: control / main / down / roster / header / status
    -> HabitatLayout
      -> SurfaceSlot
        -> SurfaceFactory
          -> AgentSessionContext / WorkspaceFileContext / BrowserContext
          -> commands, toolbar regions, status, restore data
```

Practical design implications:

- Keep persistent navigation/roster areas stable, but let selected tools move
  into main or down areas when useful.
- Make surface creation command-driven so the same operation can be invoked by
  UI, CLI, workflow, or restoration.
- Treat each provider surface as responsible for restoring its own content
  state; the shell restores placement.
- Serialize layouts as explicit workspace data rather than relying on browser
  local DOM state.
- Give terminal surfaces special runtime guarantees even if they sit inside
  the same layout model as non-terminal surfaces.
- Use a widget/surface tracker equivalent for "current agent surface",
  "current terminal", "current browser", and "current workflow graph".

## Design Principle

JupyterLab centers on computational documents and live kernels. Its
malleability comes from a restorable shell where registered widgets can occupy
known areas, not from arbitrary component movement. For Wardian, the lesson is
to make agent surfaces command-created, context-bound, and restorable inside a
stable Habitat shell.
