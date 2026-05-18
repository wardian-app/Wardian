# Dashboard

The Dashboard summarizes active agents, telemetry, and quick controls in a denser operational view than the Grid.

Use it when you want to compare agent health, spot stuck sessions, or apply quick lifecycle actions without reading every terminal.

![Wardian Dashboard view showing compact agent status rows and quick controls](../assets/screenshots/dashboard/system-summary.png)

## When to Use It

- Compare live state across several agents.
- Pause, restart, delete, or query an agent from a compact row.
- Check workspace and telemetry at a glance before opening a terminal.
- Decide whether to follow up in Grid, Queue, or Source Control.

## Basic Workflow

1. Spawn one or more agents from the [Getting Started](./getting-started.md) flow.
2. Click **Dashboard** in the top workspace tabs.
3. Scan status, hardware usage, workspace, and last activity for each active agent.
4. Use quick controls such as pause, restart, delete, or query when an agent needs attention.
5. Open the [Grid](./grid.md) when you need to inspect or type into the full terminal.

## Important Limits

- Dashboard is a summary and control surface; it does not replace terminal inspection for detailed provider output.
- Hardware numbers are best-effort telemetry and may vary by platform and provider process model.
- Deleting or restarting an agent changes runtime state. Review the selected row before using destructive controls.
- Completion summaries belong in [Queue](./queue.md); Dashboard is for live agent state.

## Related Links

- [Grid](./grid.md)
- [Watchlists](./watchlists.md)
- [Queue](./queue.md)
- [Settings](./settings.md)
