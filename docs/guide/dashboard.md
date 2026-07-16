# Dashboard

Dashboard is a Workbench surface that summarizes active agents, telemetry, and quick controls in a denser operational view than Agents.

Use it when you want to compare agent health, spot stuck sessions, or apply quick lifecycle actions without reading every terminal.

![Wardian Dashboard view showing compact agent status rows and quick controls](../assets/screenshots/dashboard/system-summary.png)

## When to Use It

- Compare live state across several agents.
- Pause, restart, delete, or query an agent from a compact row.
- Check workspace and telemetry at a glance before opening a terminal.
- Decide whether to open an agent session or place Queue, Agents, or another surface beside it.

## Basic Workflow

1. Spawn one or more agents from the [Getting Started](./getting-started.md) flow.
2. Press `Ctrl+P` / `Cmd+P`, select a pane's **+** button, or use an empty-pane Home state, then choose **Dashboard**.
3. Scan status, hardware usage, workspace, and last activity for each active agent.
4. Use quick controls such as pause, restart, delete, or query when an agent needs attention.
5. Use **Open** or **Open to Side** from the right roster when you need the full agent terminal, or open [Agents](./agents-overview.md) for multi-agent monitoring.

Dashboard is a singleton surface. Opening it again focuses its existing tab instead of creating a duplicate. You can move that tab between panes or keep it beside Queue or an agent session.

## Important Limits

- Dashboard is a summary and control surface; it does not replace terminal inspection for detailed provider output.
- Closing its tab closes only the Dashboard presentation. It does not alter any agent runtime.
- Hardware numbers are best-effort telemetry and may vary by platform and provider process model.
- Deleting or restarting an agent changes runtime state. Review the selected row before using destructive controls.
- Completion summaries belong in [Queue](./queue.md); Dashboard is for live agent state.

## Related Links

- [Workbench](./workbench.md)
- [Agents](./agents-overview.md)
- [Watchlists](./watchlists.md)
- [Queue](./queue.md)
- [Settings](./settings.md)
