# Workflow Node Reference

This page documents the current workflow node surface from a user perspective.

Two important notes:

- the **type system** contains a few node types that are not fully exposed or executed yet
- the **block names** shown in the UI matter more than the raw internal type names when you are building workflows

## Support Levels

- **Available in the builder and runtime**: you can add the node today and expect user-visible behavior.
- **Reserved / not fully wired**: the type exists, but the builder and runtime do not yet expose a complete user-facing implementation.

## Trigger and Entry Nodes

### Manual Trigger

**Status:** Available in the builder and runtime

Use it when you want a workflow to run on demand.

Behavior:

- starts the workflow immediately when launched
- can define an input schema for launch-time parameters
- outputs trigger context into the workflow registry

Common pitfall:

- forgetting that the input schema controls whether the run modal asks for manual parameters

### Scheduled Trigger

**Status:** Available in the builder and runtime

Use it when the workflow should create scheduled task instances.

Behavior:

- supports `Minutes`, `Hours`, `Daily`, `Weekly`, and `One-Time`
- creates scheduled tasks instead of immediate runs
- stores schedule metadata that appears in the sidebar

Common pitfall:

- expecting a scheduled workflow launch to behave like a live listener

### File Watcher

**Status:** Available in the builder and runtime

Use it when the workflow should react to file events.

Behavior:

- listens for matching file changes
- activates as a live listener
- appears in the sidebar under live listeners when active

Common pitfall:

- using it for time-based automation instead of a scheduled trigger

## Execution Nodes

### Agent

**Status:** Available in the builder and runtime

Use it to send a prompt into an agent session or run an agent headlessly when the target is off.

Behavior:

- supports direct targeting or role-based assignment
- supports `ephemeral`, `inherit_fresh`, and `inherit_resume` run modes
- supports `text` or `json` output formats
- can require launch-time assignment through the run modal when the run mode inherits from an existing agent
- skips automatic startup or "introduce yourself" prompts for workflow-spawned runs

Common pitfalls:

- leaving an inherited role unmapped, which prevents execution
- using `inherit_resume` when you wanted a fresh run with the selected agent's profile
- assuming JSON mode behaves exactly like an interactive PTY run

Run modes:

- `ephemeral`: use the selected class and workspace for a fresh workflow-run provider session.
- `inherit_fresh`: clone settings from the selected agent, read that agent's scoped context, and start a fresh workflow-run provider session.
- `inherit_resume`: continue the selected agent's provider session and runtime directory.

### Shell Command

**Status:** Available in the builder and runtime

Use it to run a shell command inside the workflow.

Behavior:

- executes in the configured folder
- can receive interpolated values from upstream nodes
- returns command result data, including failures

Common pitfall:

- forgetting to set the execution directory when the command depends on a specific workspace

### Script

**Status:** Available in the builder and runtime

Use it to run a local script file through a selected runtime.

Behavior:

- supports `python`, `node`, and `sh`
- resolves the script path relative to the chosen execution directory
- passes optional arguments and environment variables

Common pitfall:

- confusing a shell command with a script path; use Script for a file and Shell Command for inline command text

### Tool Call

**Status:** Reserved / not fully wired

The node type exists in the workflow model and block library, but the current runtime does not execute a distinct tool-call branch yet.

Current behavior:

- you may see the node type in the code model
- the backend does not yet expose full user-facing execution semantics for it

### Sub-Flow

**Status:** Reserved / not fully wired

The builder taxonomy includes `Sub-Flow`, but the runtime does not currently execute it as a dedicated node behavior.

Current behavior:

- useful as a planned concept in the model
- not yet a complete nested-workflow feature for users

## Control-Flow Nodes

### Branch

**Status:** Available in the builder and runtime

Use it to split flow based on a condition.

Behavior:

- evaluates a condition against registry values
- pulses either `on_true` or `on_false`

Common pitfall:

- writing a condition against a value that does not exist yet in the registry

### Loop

**Status:** Available in the builder and runtime

Use it for repeated execution.

Behavior:

- supports `count` mode and `conditional` mode
- accepts `max_iterations` as a positive integer or as a single registry
  template such as `&#123;&#123;trigger.output.limit&#125;&#125;`
- resolves `until` as a dot path against the run registry, such as
  `nodes.review.output.done`
- treats truthy `until` values as the signal to stop before another body pulse
- emits `body` while continuing and `done` when finished
- stores iterator state in workflow runtime data

Common pitfall:

- forgetting to bound the loop correctly, which can create confusing repeated behavior
- malformed `max_iterations` templates warn during validation and fall back at
  runtime instead of invalidating the whole blueprint

### Wait

**Status:** Available in the builder and runtime

Use it as a synchronization barrier.

Behavior:

- waits for the required upstream pulses before continuing
- useful after branches or parallel-looking flows that must rejoin

Common pitfall:

- treating it like a timed sleep; its main purpose is dependency synchronization

### Parallel

**Status:** Reserved / not fully wired

The type exists in the workflow model, but the current builder library and runtime do not expose it as a separate executable node.

## Coordination and State Nodes

### KV Storage

**Status:** Available in the builder and runtime

Use it to read or update shared workflow storage.

Behavior:

- supports `get`, `set`, and `delete`
- works with workspace-level or run-level state concepts in the UI
- exposes storage values back to downstream nodes

Common pitfall:

- expecting deep nested object semantics everywhere; keep storage keys simple unless you have confirmed the exact shape you need

### Notify

**Status:** Available in the builder and runtime

Use it to send a UI notification from a workflow.

Behavior:

- emits a Wardian notification message
- is best for operator-facing alerts

### Broadcast

**Status:** Partially available

Use it when you conceptually want to send a message broadly.

Current behavior:

- it is exposed in the builder
- the runtime currently treats it as a communication node, but broad agent delivery is still limited compared with the long-term intent

### Governance

**Status:** Reserved / not fully wired

The `governance` node type exists in the workflow model, but it is not currently surfaced as a full builder block with distinct runtime behavior.

## Reading Internal Types vs UI Names

A few nodes share internal types while presenting different names in the builder:

- `Notify` and `Broadcast` both use the `communication` node type
- all trigger blocks use the `trigger` node type with different names and configs

When in doubt, trust the block name you see in the Workflow Builder first, then use the internal type only when comparing against developer notes or exported JSON.

## Related References

- [Building Workflows](./building-workflows.md)
- [Triggers](./triggers.md)
- [Workflow Engine Architecture](../developer/workflow-engine.md)
