# Monitoring with Watchlists

The **Agent Watchlist (Roster)**, located in the right sidebar, is your primary tool for monitoring the health and activity of all active agents in your workspace.

## 📊 Real-Time Monitoring

### Status Indicators
Each agent in the roster has a colored status light representing its current state:
- **Emerald (Idle)**: The agent has finished its task and is waiting for input.
- **Cyan (Processing)**: The agent is actively executing a command or generating a response.
- **Amber (Action Needed)**: The agent has hit a roadblock, requires human approval, or a tool is requesting input.
- **Gray (Off)**: The agent's process has been paused or terminated.
- **Red (Error)**: A critical system or process error has occurred.

### Thought Streams
Under the agent's name, a real-time "thought stream" displays the agent's current progress (e.g., "Reading file...", "Running tests..."). This allows you to track an agent's reasoning without opening its full terminal view.

## 🗂️ Organizing Agents

As your swarm grows, you can organize agents into **Custom Watchlists** (Tabs).

### Creating and Managing Lists
1. Click the **+** icon in the Roster header to create a new list.
2. **Double-click** a tab to rename it (e.g., "Frontend", "Research").
3. **Right-click** a tab to delete the list.

### Adding Agents to Lists
- **Right-click** an agent in the "All" tab or any other list.
- Select **Add to List** and choose your target watchlist.
- An agent can belong to multiple watchlists simultaneously.

### Drag-and-Drop Reordering
- You can drag agents within a watchlist to prioritize them visually.
- Reordering in the "All" tab updates the global `agent_order` used by the Dynamic Grid.

## 🖱️ Batch Actions
- **Ctrl/Cmd + Click**: Select multiple agents to perform bulk actions (Pause, Restart, Delete) from the context menu.
- **Shift + Click**: Select a continuous range of agents.
- **Select All / Clear**: Use the footer buttons to quickly manage large selections for broadcast commands.
