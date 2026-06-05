# Workflow Node Reference

> Generated from the node type registry. Do not edit by hand.

## Task

- **id:** `task`
- **kind:** agent
- **category:** Agent
- **version:** 1

Delegate work to an agent; returns structured output.

### Fields

- `agent` — Agent [agentref (required)]
- `prompt` — Prompt [prompt (required)]
- `output_schema` — Output schema [jsonschema]

Outgoing ports: out

## Decision

- **id:** `decision`
- **kind:** agent
- **category:** Agent
- **version:** 1

Agent chooses one of the declared outgoing branches.

### Fields

- `agent` — Agent [agentref (required)]
- `prompt` — Prompt [prompt (required)]
- `choices` — Choices [branchport (required), multiple]

Outgoing ports are derived from the `choices` field.

## Branch

- **id:** `branch`
- **kind:** engine
- **category:** Control
- **version:** 1

Deterministic condition on run state.

### Fields

- `condition` — Condition [text (required)]

Outgoing ports: on_true, on_false

## Loop

- **id:** `loop`
- **kind:** engine
- **category:** Control
- **version:** 1

Container: repeats its body subgraph until a bound is hit.

### Fields

- `max_iterations` — Max iterations [number or `{{registry.path}}` template].
  Runtime clamps the resolved value to at least `1`; malformed templates warn
  during validation and fall back at runtime.
- `until` — Registry dot path checked for truthiness before another body pulse,
  for example `trigger.output.stop` or `nodes.review.output.done`.

Outgoing ports: body, done

## Join

- **id:** `join`
- **kind:** engine
- **category:** Control
- **version:** 1

Synchronization barrier; waits for all inbound edges.

_No fields._

Outgoing ports: out

## Approval

- **id:** `approval`
- **kind:** engine
- **category:** Control
- **version:** 1

Human-in-the-loop gate; parks the run until a person approves.

### Fields

- `prompt` — Approval prompt [prompt]

Outgoing ports: out

## Shell

- **id:** `shell`
- **kind:** engine
- **category:** Action
- **version:** 1

Run a shell command.

### Fields

- `command` — Command [longtext (required)]
- `cwd` — Working directory [path]

Outgoing ports: out

## Script

- **id:** `script`
- **kind:** engine
- **category:** Action
- **version:** 1

Run a local script through a selected runtime.

### Fields

- `runtime` — Runtime [enum:python|node|sh (required)]
- `path` — Script path [path (required)]

Outgoing ports: out

## State

- **id:** `state`
- **kind:** engine
- **category:** State
- **version:** 1

Read or write run or shared storage.

### Fields

- `op` — Operation [enum:get|set|delete (required)]
- `entries` — Entries [kvmap]

Outgoing ports: out

## Notify

- **id:** `notify`
- **kind:** engine
- **category:** State
- **version:** 1

Send an operator-facing notification.

### Fields

- `message` — Message [prompt (required)]

Outgoing ports: out

## Sub-workflow

- **id:** `sub_workflow`
- **kind:** engine
- **category:** Action
- **version:** 1

Call another workflow blueprint.

### Fields

- `workflow` — Workflow [workflowref (required)]

Outgoing ports: out

## Manual Trigger

- **id:** `manual_trigger`
- **kind:** trigger
- **category:** Trigger
- **version:** 1

Entry point. Runs on demand or when an invoker fires it.

### Fields

- `input_schema` — Input schema [jsonschema]

Outgoing ports: out
