# Malleable GUI References

This folder collects research notes on applications with modular, rearrangeable
workspaces. These systems are useful references for Wardian's Habitat direction:
durable surfaces, split/tab layouts, local context, visible state, and
interaction models that let users reshape the workspace without losing the
mental model.

This is not an endorsement, affiliation claim, product evaluation, or
competitive teardown. The notes below describe public architecture and design
pressure only.

Last reviewed: 2026-06-07.

## Reference Map

| System | Center of Gravity | Workspace Primitive | Wardian Takeaway |
|---|---|---|---|
| [cmux](./cmux-surface-model.md) | Multi-agent coding sessions and terminals. | Unified panels inside split/tab workspaces. | Treat terminal, browser, file, markdown, and agent surfaces as peers behind one panel protocol. |
| [Obsidian](./obsidian-draggable-workspace.md) | Markdown files and plugin views. | Workspace leaves inside splits, side docks, tabs, and pop-out windows. | Separate durable layout slots from view state so panes can move without each view owning layout logic. |
| [VS Code](./vscode-workbench-layout.md) | Code files and extension views. | Editor groups plus constrained workbench regions. | Use stable placement zones and registry services to make customization predictable rather than unbounded. |
| [JupyterLab](./jupyterlab-computational-workspace.md) | Notebooks, kernels, terminals, and computational documents. | Lumino widgets inside named shell areas and a restorable dock panel. | Use command-created, context-bound surfaces with plugin-owned restoration and shell-owned placement. |
| [Blender](./blender-workspace-area.md) | Scene/object work and task workspaces. | Screen areas hosting registered editor types and regions. | Make the manipulable unit a durable surface area with internal regions, not arbitrary floating components. |

## Cross-System Pattern

The strongest examples do not make every UI component equally draggable. They
choose a durable workspace primitive and make that primitive feel physical:

- cmux: panel/surface.
- Obsidian: workspace leaf.
- VS Code: editor group or contributed view.
- JupyterLab: shell widget or document widget.
- Blender: editor area.

Across the four systems, the useful design pattern for Wardian is:

```text
workspace
  -> layout graph
    -> durable surface slot
      -> registered surface/view/editor type
        -> local regions, state, status, commands, and drag/drop policy
```

This points Wardian toward a Habitat surface model where agent terminals,
browsers, files, transcripts, diffs, task plans, workflow graphs, and status
inspectors share one layout vocabulary while still preserving each surface's
native behavior.

## Wardian Design Implications

- Define one durable surface-slot model before adding more specialized views.
- Keep surface placement, focus, split, tab, move, close, restore, and status
  behavior outside individual surface implementations.
- Let surface types register capabilities, commands, status, keybindings,
  drag/drop payloads, and region definitions.
- Preserve per-surface-type state when a slot is retargeted or temporarily
  hidden.
- Use constrained layout affordances before arbitrary floating windows.
- Keep the active agent/task/session as the shared context center, analogous to
  Blender's scene/object context or Obsidian's active file context.
- Scope attention and redraw to surface regions so status changes do not all
  compete at the same visual level.

## Open Questions

- Should Wardian's first durable primitive be a `SurfaceArea`, a `Panel`, or a
  `WorkspaceLeaf`-style slot?
- Which surfaces need native fidelity rather than generic web rendering:
  terminal, browser, file preview, diff, workflow graph, or telemetry?
- How much layout mobility should the right roster and left navigation rail
  have before they harm repeat-use muscle memory?
- Should surface layouts be saved as markdown-adjacent JSON, TOML, or a Rust
  DTO serialized through the backend state model?
