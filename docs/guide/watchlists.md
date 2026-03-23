# Monitoring with Watchlists

The **Agent Watchlist** (Right Sidebar) is your primary high-fidelity tool for monitoring the health, activity, and thoughts of your agent swarm.

## 🕵️ Real-Time Monitoring

### 1. Status Indicators
Every agent in the watchlist has a distinct status light:
- **Emerald (Idle)**: Ready for a new task.
- **Cyan (Processing)**: Currently executing or thinking.
- **Amber (Action Needed)**: Waiting for user confirmation or a tool prompt.
- **Gray (Off)**: Session is paused or hibernating.
- **Red (Error)**: Encountered a fatal process error.

### 2. Live Thought Bubbles
Wardian captures the agent's internal telemetry and displays it as a "Thought Bubble" next to the agent's name. This allows you to see what the agent is currently working on without reading the full terminal log.

## 🗂️ Organizing with Watchlists
As your swarm grows, a single list becomes difficult to manage. Wardian allows you to group agents into custom **Watchlists**.

### Creating a Watchlist
1. Click the **+** icon at the top of the Right Sidebar.
2. Give your list a name (e.g., "Frontend Ops").
3. Your new list will appear as a tab or filter in the roster.

### Managing Agents
- **Reordering**: Drag and drop agent cards within a watchlist to prioritize your view.
- **Filtering**: Click a watchlist tab to focus only on that group of agents. 
- **Bulk Selection**: Use `Ctrl+Click` to select multiple agents within a watchlist for broadcast commands.

## 🖱️ Remote Management
Hover over any agent in the Roster to access instant control icons:
- **Pause/Resume**: Suspend the PTY process to save CPU.
- **Restart**: Re-spawn the agent with its initial instructions.
- **Quick-Jump**: Double-click an agent to center the main Grid view on that agent's terminal.
