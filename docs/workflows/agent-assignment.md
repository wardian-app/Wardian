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

## Agent Run Modes

Agent nodes use one run mode selector:

- **Ephemeral**: build a fresh workflow execution from an agent class and workspace. It does not need launch-time agent assignment.
- **Inherit Fresh**: clone provider, class, workspace, skill, and scoped-memory read configuration from an existing agent, but start a fresh provider session for this workflow run.
- **Inherit Resume**: continue the selected agent's provider session and mutable runtime state. Use this only when the workflow should deliberately add to that agent's conversation history.

From a user perspective, the important distinction is whether the workflow needs an existing agent. Ephemeral runs do not. Inherited runs do, so they can appear in the Agent Assignments section when no direct agent is already selected.

Workflow-spawned agent runs do not receive an automatic "introduce yourself" startup prompt. The first provider input is the workflow node prompt.

## Off Agents and Headless Execution

If a target agent is off, Wardian can still execute the workflow through headless provider logic instead of requiring the terminal to be visibly open.

What users should expect:

- the workflow still attempts to run if the provider supports headless execution
- role mappings still matter for inherited runs even if the target agent is not currently open in a visible terminal
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

- the workflow should inherit from or resume one specific long-lived agent
- reusability is not important for that automation

Prefer **Inherit Fresh** when you want an existing agent's profile without conversation-history token growth. Reserve **Inherit Resume** for workflows whose purpose is to continue that exact agent session.

The global regular-agent session setting does not change workflow Agent node behavior. Workflow runs follow the node's run mode.

## Related References

- [Triggers](./triggers.md)
- [Scheduled Runs](./scheduled-runs.md)
- [Provider Runtime Notes](../developer/provider-runtimes.md)
