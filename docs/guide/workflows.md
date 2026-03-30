# Workflow View

The Workflow view is Wardian's canvas for building and testing automations.

Use this page as a quick manual for the view itself. For the full workflow reference, start at [Workflows](../workflows/index.md).

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

## Where to Learn More

- [Workflows](../workflows/index.md)
- [Building Workflows](../workflows/building-workflows.md)
- [Triggers](../workflows/triggers.md)
- [Scheduled Runs](../workflows/scheduled-runs.md)
- [Node Reference](../workflows/node-reference.md)
