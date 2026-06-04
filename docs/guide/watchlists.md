# Monitoring with Watchlists

The **Agent Watchlist** (Right Sidebar) is your primary high-fidelity tool for monitoring the health, activity, and thoughts of your agent swarm.

Use it when you need persistent awareness of all agents while working in Grid, Dashboard, Library, Workflows, or any sidebar tool.

![Wardian agent roster showing grouped agents with status, query count, and last queried columns](../assets/screenshots/watchlists/agent-roster.png)

## When to Use It

- Select one or more agents before using Command, Library prompt runs, Explorer, or Source Control.
- Group agents by project, workstream, role, or review lane.
- Spot agents that are idle, processing, blocked, off, or errored without opening every terminal.
- Jump back to a specific terminal in the [Grid](./grid.md).

## Basic Workflow

1. Spawn agents from [Getting Started](./getting-started.md) or the left Agent Configuration tab.
2. Use the roster to select the agents you want to inspect or target.
3. Sort or filter the list when the swarm grows.
4. Create watchlists or teams for repeated groups.
5. Double-click an agent to focus it in Grid, or use context actions for lifecycle controls.

## Real-Time Monitoring

### Status Indicators
Every agent in the watchlist has a distinct status light:
- **Emerald (Idle)**: Ready for a new task.
- **Cyan (Processing)**: Currently executing or thinking.
- **Amber (Action Needed)**: Waiting for user confirmation or a tool prompt.
- **Gray (Off)**: Session is paused or hibernating.
- **Red (Error)**: Encountered a fatal process error.

### Live Thought Bubbles
Wardian captures the agent's internal telemetry and displays it as a "Thought Bubble" next to the agent's name. This allows you to see what the agent is currently working on without reading the full terminal log.

## Customizable Columns

Click the **gear icon** (⚙) in the watchlist header to open the column picker. Each column can be toggled on or off independently:

| Column | Default | Description |
|---|---|---|
| Status | On | Current agent status label |
| Query Count | On | Number of prompts sent this session |
| Uptime | Off | Time since the agent process started |
| Provider / Model | Off | Provider name and model identifier |
| Last Queried | On | Time elapsed since the last prompt was sent |

### Sorting
Click any column header to sort by that column. Clicking again cycles through ascending → descending → unsorted. The **Agent** column header sorts alphabetically by name. Sorting applies on top of your custom watchlist order; drag-to-reorder still works when no sort is active.

### Persistence
Column visibility and sort state are saved to `<wardian-home>/watchlists/prefs.json` and restored on next launch.

New visible agents normally appear at the top of the roster. Change
**Settings > Watchlist > New agent position** to place agents spawned from the
app or with `wardian agent spawn` at the bottom instead. This setting affects
explicit new spawns only; existing roster order, manual drag order, clone
placement, teams, and custom watchlist entries keep their own ordering rules.

The CLI can inspect persisted watchlist and team state without starting the desktop app:

```bash
wardian team list
wardian team show <team-name-or-id>
wardian watchlist list
wardian watchlist show <watchlist-name-or-id>
```

These commands read the same `watchlists/index.json` file as the GUI and accept both the current v2 state shape with global teams and legacy flat watchlist arrays. They are read-only; team mutation and team send targeting are planned as separate CLI slices.

## Organizing with Watchlists
As your swarm grows, a single list becomes difficult to manage. Wardian allows you to group agents into custom **Watchlists**.

### Creating a Watchlist
1. Click the **+** icon at the top of the Right Sidebar.
2. Give your list a name (e.g., "Frontend Ops").
3. Your new list will appear as a tab or filter in the roster.

### Managing Agents
- **Reordering**: Drag and drop agent cards within a watchlist to prioritize your view.
- **Filtering**: Click a watchlist tab to focus only on that group of agents.
- **Bulk Selection**: Use `Ctrl+Click` to select multiple agents within a watchlist for broadcast commands.
- **Bulk Context Menu**: If you right-click inside the current multi-selection, the menu applies to the whole selection. Bulk delete shows one confirmation dialog for the full selected set instead of prompting once per agent.

### Collapsing Teams
Click the chevron in a team header to hide or reveal that team's members. The team header stays visible with its member count, and context-menu actions still apply to the full team. Collapsed teams are saved with the rest of the watchlist display preferences and restored on next launch.

![Wardian watchlist showing one collapsed team and one expanded team](../assets/screenshots/watchlists/team-collapse.png)

Teams are also Wardian's project/workstream grouping concept. They are useful
when a line of work spans more than one workspace or folder, or when one
workspace contains several parallel efforts. Watchlists decide what is visible
and targetable now; teams describe the durable work context those agents are
cooperating inside.

## Remote Management
Hover over any agent in the Roster to access instant control icons:
- **Pause/Resume**: Suspend the PTY process to save CPU.
- **Restart**: Re-spawn the agent with its initial instructions.
- **Quick-Jump**: Double-click an agent to center the main Grid view on that agent's terminal.

## Important Limits

- The roster is the targeting surface for many tools. Check selection before broadcasting or running prompts.
- CLI team and watchlist commands are currently read-only for persisted state.
- Status and thought snippets are compact summaries. Use Grid or the CLI watch command for detailed output.

## Related Links

- [Grid](./grid.md)
- [Dashboard](./dashboard.md)
- [Command Panel](./command-panel.md)
- [Wardian CLI](./cli.md)
