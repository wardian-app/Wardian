# Workflows

Wardian workflows are reusable automations built from nodes, connections, and runtime assignment data. This section is the main user-facing reference for building workflows, understanding how they launch, and managing scheduled or live workflow behavior over time.

## Workflow Mental Model

A workflow has three layers:

- **Template**: the saved graph of nodes, edges, and workflow settings.
- **Launch behavior**: whether that template runs manually, creates a scheduled task, or starts a live listener.
- **Runtime state**: active runs, scheduled instances, listener status, node outputs, and role mappings.

In practice, you usually move through workflows in this order:

1. Build or edit the graph in the Workflow view.
2. Save and launch it from either the main builder or the sidebar library.
3. Monitor active runs, live listeners, and scheduled tasks from the workflow sidebar.

## Start Here

- **[Building Workflows](./building-workflows.md)**: Use the canvas, block library, node settings, and variable assistant.
- **[Triggers](./triggers.md)**: Understand manual runs, scheduled triggers, and live listeners.
- **[Scheduled Runs](./scheduled-runs.md)**: Manage scheduled task instances, pause/resume, run now, and deletion.
- **[Node Reference](./node-reference.md)**: Reference every current workflow node type and its user-visible behavior.
- **[Agent Assignment](./agent-assignment.md)**: Learn how roles, direct agent selection, and the run modal work.
- **[Troubleshooting](./troubleshooting.md)**: Diagnose the most common workflow problems quickly.

## Workflow Surfaces

Wardian exposes workflows in three main surfaces:

- **Workflow Builder**: best for authoring, wiring, configuring, and testing workflows.
- **Workflow Library**: best for launching saved workflows quickly without opening the canvas.
- **Active Monitoring**: best for watching active runs, live listeners, and scheduled task instances.

## What This Section Covers

This workflow section focuses on user-visible behavior:

- what each node is for
- when a run modal appears
- how scheduled tasks behave
- what happens when a workflow is launched from different surfaces
- how agent assignments affect execution

For backend implementation details, see:

- [Workflow Engine Architecture](../developer/workflow-engine.md)
- [Visual Builder Architecture](../developer/visual-builder.md)
