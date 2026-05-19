# Workflow View

The Workflow view is Wardian's canvas for building and testing automations.

Use this page as a quick manual for the view itself. For the full workflow reference, start at [Workflows](../workflows/index.md).

Use it when a repeated multi-step agent process needs a saved visual flow instead of a one-off prompt or broadcast.

![Wardian Workflow view showing the workflow sidebar, builder canvas, connected nodes, and run controls](../assets/screenshots/workflows/builder-canvas.png)

## When to Use It

- Chain multiple agent, command, wait, or branch steps.
- Reuse a process that should run manually, on a schedule, or from a listener trigger.
- Inspect upstream values before passing them into later nodes.
- Compare workflow outcomes in [Queue](./queue.md).

## What You Can Do Here

- open an existing workflow
- create a new workflow
- add nodes from the block library
- wire nodes together on the canvas
- edit node settings
- save changes
- launch the workflow according to its trigger type

## Main Areas

- **Top action bar**: select, save, reset, duplicate, delete, or run the active workflow
- **Canvas**: place and connect nodes
- **Block Library**: add new blocks to the workflow
- **Node Settings drawer**: edit the selected node
- **Variable Assistant**: inspect upstream values and interpolation paths

## Running From This View

The **Run Workflow** button saves the current canvas first, then launches based on the workflow's trigger:

- manual workflows run immediately
- scheduled workflows create scheduled task instances
- file-watcher or webhook-style workflows activate as live listeners

If the workflow needs agent assignment or manual input parameters, Wardian opens the run modal before launching.

## Important Limits

- The visual view is for building and launching workflows. Detailed node semantics live in the workflow reference.
- Real provider behavior still depends on the selected agent class, provider CLI, workspace, and runtime settings.
- Scheduled and listener workflows require the app runtime to be available when they are expected to run.
- Queue records final workflow outcomes, not every intermediate node state.

## Related Links

- [Getting Started](./getting-started.md)
- [Workflows](../workflows/index.md)
- [Building Workflows](../workflows/building-workflows.md)
- [Triggers](../workflows/triggers.md)
- [Scheduled Runs](../workflows/scheduled-runs.md)
- [Node Reference](../workflows/node-reference.md)
