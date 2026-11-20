# Agent Assignment

Automations can target specific agents directly or ask you to assign agents at launch time.

The assignment model is built around **roles**.

## Roles vs Direct Agent IDs

An Agent node can target work in two different ways:

- **Direct agent ID**: the automation is already tied to a specific agent
- **Role mapping**: the automation defines a role name, and you choose which live agent should fill that role when you launch it

Wardian normalizes agent roles before launch so an automation can be reused without hardcoding the same session ID forever.

## When the Run Modal Appears

The run modal appears when the automation needs launch-time configuration.

Common reasons:

- at least one Agent node needs assignment
- the Manual Trigger defines an input schema
- a scheduled automation needs both assignment and schedule creation in the same launch flow

The modal can show two sections:

- **Agent Assignments**
- **Input Parameters**

If neither section is needed, Wardian skips the modal and launches directly.

## What the Assignment Section Shows

For each agent role, the modal shows:

- the node name from the automation graph
- the internal role key
- a selector for the target live agent

This lets one automation template be reused across different agent rosters or different scheduled instances.

## Agent Run Modes

Agent nodes use one run mode selector:

- **Ephemeral**: build a fresh automation execution from an agent class and workspace. It does not need launch-time agent assignment.
- **Inherit Fresh**: clone provider, class, workspace, skill, and scoped-memory read configuration from an existing agent, but start a fresh provider session for this automation run.
- **Inherit Resume**: continue the selected agent's provider session and mutable runtime state. Use this only when the automation should deliberately add to that agent's conversation history.

From a user perspective, the important distinction is whether the automation needs an existing agent. Ephemeral runs do not. Inherited runs do, so they can appear in the Agent Assignments section when no direct agent is already selected.

Automation-spawned agent runs do not receive an automatic "introduce yourself" startup prompt. The first provider input is the automation node prompt.

## Off Agents and Headless Execution

If a target agent is off, Wardian can still execute the automation through headless provider logic instead of requiring the terminal to be visibly open.

What users should expect:

- the automation still attempts to run if the provider supports headless execution
- role mappings still matter for inherited runs even if the target agent is not currently open in a visible terminal
- provider-specific quirks can affect the outcome, especially for structured output or approvals

## Scheduled Assignment

When you create a scheduled task, the chosen role mappings become part of that scheduled instance.

That is why:

- multiple schedules of the same automation can target different agents
- the sidebar can summarize the target for each schedule separately
- deleting a scheduled task does not change the automation template itself

## Practical Guidance

Use **roles** when:

- the automation should be reusable
- different teams or classes may fill the same responsibility
- you expect to create multiple scheduled variants

Use **direct agent targeting** when:

- the automation should inherit from or resume one specific long-lived agent
- reusability is not important for that automation

Prefer **Inherit Fresh** when you want an existing agent's profile without conversation-history token growth. Reserve **Inherit Resume** for automations whose purpose is to continue that exact agent session.

With Codex, both modes use the selected agent's MCP server settings, including
each server's enabled tools. **Inherit Fresh** applies those settings to a
separate run-owned Codex home. Temporary provider runs use the upstream Codex
configuration.

The global regular-agent session setting does not change automation Agent node behavior. Automation runs follow the node's run mode.

## Related References

- [Triggers](./triggers.md)
- [Scheduled Runs](./scheduled-runs.md)
- [Provider Runtime Notes](../developer/provider-runtimes.md)
