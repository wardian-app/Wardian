# Building Automations

Use the Automation Builder when you want to create, edit, or test automation logic directly on the canvas.

The canvas is the authoring surface; the Rust automation engine is the runtime. When you run an automation, Wardian saves the graph, resolves launch-time input, then processes candidate nodes through the engine's internal execution loop, records node output, pulses downstream ports, and emits run telemetry.

## Builder Layout

The Automation view is made of four main working areas:

- **Automation selector and action bar**: choose a saved automation, create a new one, save changes, reset, duplicate, delete, or run.
- **Canvas**: place nodes and draw connections between them.
- **Block Library**: add new nodes to the graph.
- **Node Settings drawer**: configure the currently selected node.

The builder also includes the **Variable Assistant**, which shows upstream values you can interpolate into prompts, conditions, paths, and commands.

## Basic Authoring Flow

1. Create or open an automation.
2. Add nodes from the Block Library.
3. Connect outputs to downstream inputs.
4. Configure each node in the right-side settings drawer. Branch conditions
   use a dot-separated registry path; comparison expressions are rejected.
5. Save changes.
6. Run the automation or activate its trigger behavior.

## Working With Nodes

When you click a node, Wardian opens the node settings drawer. That drawer shows the fields for the selected block type and hides fields that do not apply to the current mode.

Examples:

- Manual Trigger can declare an input schema for the run modal.
- Agent nodes resolve their configured role/class or invocation binding.
- Loop's optional `until` field uses the same registry-path condition grammar as Branch.

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
- **Save Changes**: persist the current automation graph.
- **Run Automation**: save first, then launch based on the automation's trigger type.

Builder launch behavior is intentionally invoker-aware:

- automations with a **Manual Trigger** run immediately
- schedules are created separately and invoke the saved blueprint later
- event listeners, when available, invoke the same durable run contract

## When the Run Modal Appears

Wardian opens the run modal when the automation needs extra launch-time input.

That usually means one or both of these are true:

- the automation has a **Manual Trigger** with an input schema
- the automation contains **Agent** nodes in an inherited run mode that need role-to-agent assignments

If neither is needed, the automation launches immediately based on its trigger type.

After an automation finishes, the app can add its completed or failed outcome to the separate [Inbox](../guide/inbox.md) triage surface.

## Builder vs Library

Use the **Automation Builder** when you need to:

- change the graph
- edit node settings
- test an automation while looking at the canvas
- confirm exactly what will be saved before launch

Use the **Automation Library** when you need to:

- launch an existing automation quickly
- inspect the saved blueprint before creating an invoker

`sub_automation` is reserved and does not appear in the Block Library. Existing
blueprints containing it fail validation with `unsupported_node_type`; this is
intentional until durable child-run input, provenance, approval, cancellation,
and restart semantics are implemented.

## Delivering an Artifact as a Message

Use `message_send` for host-owned informational delivery after a task writes its
result. It does not create a task or require a reply. The application host must
be available; a standalone headless executor cannot deliver messages.

```yaml
- id: deliver-review
  type: message_send
  fields:
    recipient: '{{trigger.output.requesting_agent}}'
    artifact_path: '.wardian-review/{{run.id}}/review.md'
```

`recipient` accepts one exact agent UUID or unambiguous exact name. Class and
broadcast selectors are rejected. The canonical receipt records the resolved
UUID as `recipient_id`. Preflight persists that UUID and the rendered artifact path
in a `message_send_prepared` run event before review. Renaming or reassigning the
original name cannot retarget delivery or resume; deletion of the bound agent
fails closed. Use the same artifact path in the producing task's prompt.
`run.id` is reserved, restored from the persisted run identity on replay and
resume, and cannot be replaced by trigger or task output.

Before tasks start, the engine checks host capability, resolves delivery fields
and the recipient, and prepares the artifact directory. Delivery fields support
literals, `trigger.output` values and `run.id`; task-output references are rejected.
An existing artifact fails fresh-run preflight. Resume retains the run identity
and artifact; use a run-specific directory to keep concurrent runs separate.

The path must be workspace-relative, use forward slashes, and contain no traversal,
links or reparse points. Delivery reads one regular UTF-8 file of at most 64 KiB.
Missing, unreadable, invalid UTF-8, blank and oversized artifacts fail delivery.
No newline, BOM, whitespace, template or shell transformation is applied to the
body. The receipt contains `interaction_id`, `run_id`, `node`, `recipient_id`,
`artifact_path`, `artifact_sha256`, `idempotency_key`, `delivery_state` and `duplicate`.
The SHA256 covers the exact body bytes. Initial admission state is `stored`;
success proves durable recipient-addressed admission, not consumption or reading.

The canonical interaction store owns the message and host provenance. The run
records a receipt, while the workspace retains the recoverable artifact. A stable
run/node key makes identical replay return the original admission; changed body
or resolved recipient conflicts. Do not place a changing recipient or body behind
the same delivery node in a loop. Native information delivery uses existing
durable claims after admission and does not schedule a task.

Admission failure fails the run and prevents downstream success notifications;
completed review output and its artifact remain. Crash recovery reconciles the
same key and provenance before reporting the original receipt. An unreadable or
missing artifact still fails even if an earlier admission exists; the error
reports its ID when available. An unresolved storage outcome remains a failure.
Failed runs are not automatically retried, and review verdicts do not gate delivery.

## Related References

- [Triggers](./triggers.md)
- [Node Reference](./node-reference.md)
- [Agent Assignment](./agent-assignment.md)
- [Inbox](../guide/inbox.md)
- [Visual Builder Architecture](../developer/visual-builder.md)
