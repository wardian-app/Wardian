# Getting Started with Wardian

Wardian is a unified terminal manager for multiple autonomous agent sessions. This guide will walk you through spawning your first agent and navigating the dual-sidebar layout.

## 1. Prerequisites
- **Node.js** (v18+)
- **Rust** (v1.75+)
- **Gemini CLI**: Install globally via `npm install -g @google/gemini-cli`.

## 2. Spawning Your First Agent
1. Open Wardian and navigate to the **Left Sidebar (Explorer)**.
2. Select an **Agent Class** (e.g., `Coder` or `Architect`).
3. Click **Spawn Instance**.
4. Your new agent will appear in the **Right Sidebar (Roster)** and automatically take up a slot in the **Dynamic Grid**.

## 3. The Dual-Sidebar Layout
- **Left Sidebar (Control)**: Use this for switching between views like the **Explorer** (for spawning agents), **Workflows** (for visual building), and **Settings**.
- **Right Sidebar (Roster)**: This is your command center for active agents.
    - **Search**: Quickly find agents by session ID or display name.
    - **Status Indicators**: Monitor if an agent is **Idle**, **Processing**, or requires **Action**.
    - **Drag & Drop**: Reorder agents to change their position in the main grid view.

## 4. Basic Agent Actions
From the agent's header in the main grid or the right sidebar, you can:
- **Pause**: Suspend the terminal process while preserving its context.
- **Restart**: Reset the PTY session to its initial state.
- **Delete**: Permanently terminate the agent and remove it from the roster.
- **Query**: Use the rapid context extraction tool to prompt the agent without typing into the terminal.

## Next Steps
- Learn how to build [Visual Workflows](./workflows.md).
- Explore the [Agent Roles](../agents/roles.md) and capabilities.
