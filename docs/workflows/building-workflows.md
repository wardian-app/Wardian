# Building Workflows

Use the Workflow Builder when you want to create, edit, or test workflow logic directly on the canvas.

## Builder Layout

The Workflow view is made of four main working areas:

- **Workflow selector and action bar**: choose a saved workflow, create a new one, save changes, reset, duplicate, delete, or run.
- **Canvas**: place nodes and draw connections between them.
- **Block Library**: add new nodes to the graph.
- **Node Settings drawer**: configure the currently selected node.

The builder also includes the **Variable Assistant**, which shows upstream values you can interpolate into prompts, conditions, paths, and commands.

## Basic Authoring Flow

1. Create or open a workflow.
2. Add nodes from the Block Library.
3. Connect outputs to downstream inputs.
4. Configure each node in the right-side settings drawer.
5. Save changes.
6. Run the workflow or activate its trigger behavior.

## Working With Nodes

When you click a node, Wardian opens the node settings drawer. That drawer shows the fields for the selected block type and hides fields that do not apply to the current mode.

Examples:

- Scheduled Trigger shows different fields depending on whether you pick `Minutes`, `Hours`, `Daily`, `Weekly`, or `One-Time`.
- Agent shows different targeting fields depending on whether the session type is `persistent` or `temporary`.
- Loop shows different fields for `count` versus `conditional` mode.

## Connections and Flow

Nodes run based on their incoming dependencies and output ports.

Common patterns:

- connect a trigger into an execution node to start work
- connect a `Branch` into different follow-up paths
- connect a `Loop` body back into downstream work and let `done` exit the cycle
- use `Wait` when multiple branches need to synchronize before continuing

## Save, Reset, and Run

The builder has three important actions:

- **Reset**: discard unsaved canvas changes and reload the saved version.
- **Save Changes**: persist the current workflow graph.
- **Run Workflow**: save first, then launch based on the workflow's trigger type.

Builder launch behavior is intentionally type-aware:

- workflows with a **Manual Trigger** run immediately
- workflows with a **Scheduled Trigger** create a scheduled task instance
- workflows with a **File Watcher** or webhook-style listener activate a live listener instead of doing a one-off run

## When the Run Modal Appears

Wardian opens the run modal when the workflow needs extra launch-time input.

That usually means one or both of these are true:

- the workflow has a **Manual Trigger** with an input schema
- the workflow contains **Agent** nodes that need role-to-agent assignments

If neither is needed, the workflow launches immediately based on its trigger type.

## Builder vs Library

Use the **Workflow Builder** when you need to:

- change the graph
- edit node settings
- test a workflow while looking at the canvas
- confirm exactly what will be saved before launch

Use the **Workflow Library** when you need to:

- launch an existing workflow quickly
- create another scheduled instance of a saved workflow
- start or reactivate a listener without opening the graph

## Related References

- [Triggers](./triggers.md)
- [Node Reference](./node-reference.md)
- [Agent Assignment](./agent-assignment.md)
- [Visual Builder Architecture](../developer/visual-builder.md)
