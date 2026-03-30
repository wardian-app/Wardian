# Agent Assignment

Workflows can target specific agents directly or ask you to assign agents at launch time.

The assignment model is built around **roles**.

## Roles vs Direct Agent IDs

An Agent node can target work in two different ways:

- **Direct agent ID**: the workflow is already tied to a specific agent
- **Role mapping**: the workflow defines a role name, and you choose which live agent should fill that role when you launch it

Wardian normalizes agent roles before launch so a workflow can be reused without hardcoding the same session ID forever.

## When the Run Modal Appears

The run modal appears when the workflow needs launch-time configuration.

Common reasons:

- at least one Agent node needs assignment
- the Manual Trigger defines an input schema
- a scheduled workflow needs both assignment and schedule creation in the same launch flow

The modal can show two sections:

- **Agent Assignments**
- **Input Parameters**

If neither section is needed, Wardian skips the modal and launches directly.

## What the Assignment Section Shows

For each agent role, the modal shows:

- the node name from the workflow graph
- the internal role key
- a selector for the target live agent

This lets one workflow template be reused across different agent rosters or different scheduled instances.

## Persistent vs Temporary Agent Nodes

Agent nodes support two user-facing modes in the builder:

- **Persistent**: target a real agent session that already exists
- **Temporary**: choose an agent class and workspace so Wardian can execute against a temporary agent context

From a user perspective, the important difference is that persistent mode is about targeting an existing agent, while temporary mode is about launching work from a class-based configuration.

## Off Agents and Headless Execution

If a target agent is off, Wardian can still execute the workflow through headless provider logic instead of requiring the terminal to be visibly open.

What users should expect:

- the workflow still attempts to run if the provider supports headless execution
- role mappings still matter even if the target agent is not currently open in a visible terminal
- provider-specific quirks can affect the outcome, especially for structured output or approvals

## Scheduled Assignment

When you create a scheduled task, the chosen role mappings become part of that scheduled instance.

That is why:

- multiple schedules of the same workflow can target different agents
- the sidebar can summarize the target for each schedule separately
- deleting a scheduled task does not change the workflow template itself

## Practical Guidance

Use **roles** when:

- the workflow should be reusable
- different teams or classes may fill the same responsibility
- you expect to create multiple scheduled variants

Use **direct agent targeting** when:

- the workflow truly belongs to one specific long-lived agent
- reusability is not important for that automation

## Related References

- [Triggers](./triggers.md)
- [Scheduled Runs](./scheduled-runs.md)
- [Provider Runtime Notes](../developer/provider-runtimes.md)
