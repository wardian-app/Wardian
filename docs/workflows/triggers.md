# Triggers

Triggers decide how a workflow enters the runtime.

In Wardian, not every workflow launch means the same thing. A launch can:

- run immediately
- create a scheduled task
- activate a live listener

The trigger nodes in the workflow decide which one happens.

## Manual Trigger

Use **Manual Trigger** when you want an on-demand workflow.

Best for:

- testing a workflow from the builder
- ad hoc automations
- workflows that should only run when a user explicitly starts them

Behavior:

- launching the workflow starts a run immediately
- if the manual trigger defines an input schema, the run modal asks for those values first
- if the workflow also contains agent roles, the same modal can collect agent assignments

## Scheduled Trigger

Use **Scheduled Trigger** when you want Wardian to create a scheduled task instance.

Best for:

- recurring reviews
- timed maintenance tasks
- delayed one-time automations
- repeating agent routines

Behavior:

- launching from the **sidebar library** creates a new scheduled task instance
- launching from the **main builder** also creates a scheduled task instance after saving first
- launching a scheduled workflow does **not** create a live listener
- a workflow can have multiple scheduled task instances at the same time

If the workflow contains agent nodes or a manual input schema, Wardian opens the run modal before creating the schedule so you can set runtime assignments.

### Schedule Types

The current scheduled trigger supports:

- **Minutes**
- **Hours**
- **Daily**
- **Weekly**
- **One-Time**

User-visible timing rules:

- interval schedules such as `Minutes` and `Hours` schedule the first run **after** the interval elapses, not immediately
- `Daily` and `Weekly` wait for the next matching wall-clock time
- `One-Time` runs once at the specified datetime and then disappears after completion

## File Watcher and Listener-Style Triggers

Wardian currently treats file watching and webhook-style triggers as **live listeners**.

Behavior:

- launching them activates the workflow instead of running it immediately
- active listener workflows appear in the **Live Listeners** section of the sidebar
- stopping them disables the active trigger instead of deleting the workflow

Use listener triggers for:

- file-change automation
- event-driven workflows that should keep watching for input

Do **not** use scheduled triggers when you really want an always-on listener. Scheduled workflows and live listeners are distinct runtime behaviors.

## Launch Surface Differences

The trigger type matters more than the button you clicked, but the surface still affects the flow:

- **Builder**: saves current canvas state first, then launches
- **Library**: launches the saved workflow directly
- **Monitoring sidebar**: acts on existing runtime instances such as listeners and scheduled tasks

## Practical Rule of Thumb

- want one run right now: use **Manual Trigger**
- want repeated or delayed runs: use **Scheduled Trigger**
- want an always-on background watcher: use **File Watcher** or webhook-style listener

## Related References

- [Scheduled Runs](./scheduled-runs.md)
- [Agent Assignment](./agent-assignment.md)
- [Workflow Engine Architecture](../developer/workflow-engine.md)
