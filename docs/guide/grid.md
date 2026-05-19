# Grid

The Grid is Wardian's primary live workspace for interacting with active agent terminals.

Use it when you need to watch multiple agents at once, type directly into a specific agent, or keep terminal state visible while agents run.

![Wardian Grid view showing the left control rail, active agent cards, and right agent roster](../assets/screenshots/grid/app-shell.png)

## When to Use It

- Watch active terminals side by side.
- Type directly into one provider session.
- Reorder agent cards to match the work you are supervising.
- Jump from the roster to the matching terminal.

## Basic Workflow

1. Start with [Getting Started](./getting-started.md) if you have not spawned an agent yet.
2. Click **Grid** in the top workspace tabs.
3. Select one or more agents in the roster when you want sidebar tools to target them.
4. Click inside an agent terminal to type directly to that session.
5. Drag agent cards to reorder the workspace when you need a different visual priority.
6. Double-click an agent in the roster to bring that agent into view.

## Important Limits

- Grid only shows agents that have active or restorable Wardian sessions.
- Provider approval prompts still belong to the provider terminal experience; Queue is for completed outcomes, not live approval.
- Terminal state is preserved across common remounts, but provider TUIs can still repaint after resize or reconnect events.
- Use [Command Panel](./command-panel.md) for repeatable fan-out messages instead of typing the same text into each terminal.

## Related Links

- [Dashboard](./dashboard.md)
- [Watchlists](./watchlists.md)
- [Command Panel](./command-panel.md)
- [Queue](./queue.md)
