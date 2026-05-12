# Getting Started with Wardian

Wardian is an integrated habitat for managing multiple autonomous agents. This guide will help you spawn your first agent and navigate the "Blueprint" system.

## 1. Prerequisites
- **Node.js** (v18+)
- **Rust** (v1.75+)
- **Gemini CLI**: Install globally via `npm install -g @google/gemini-cli`.

## 2. Understanding Blueprints
Before spawning an agent, you must understand **Classes**. A Class is a "Blueprint" that defines an agent's base instructions and capabilities.
1. Click **LIBRARY** in the top bar view-switcher.
2. Select the **Classes** tab.
3. Browse the default classes (e.g., `Architect`, `Coder`, `Researcher`). Each class has its own `AGENTS.md` instruction set.

## 3. Spawning Your First Agent

![Wardian spawn agent form with agent name, class, workspace, provider, and initialize controls](../assets/screenshots/spawn-agent/spawn-form.png)

1. Navigate to the **Left Sidebar (Agent Configuration tab)**.
2. Select an **Agent Class** from the dropdown.
3. Give your agent a unique name (e.g., "Main Researcher").
4. Click **Spawn Instance**.
5. Your new agent will appear in the **Right Sidebar (Roster)** and automatically take up a slot in the **Grid View**.

## 4. Basic Agent Management
From the **Roster (Right Sidebar)**, you can monitor and control your agents:
- **Status Lights**: 
    - **Emerald**: Idle (waiting for input).
    - **Cyan**: Processing (thinking/executing).
    - **Amber**: Action Required (needs your approval or input).
    - **Red**: Error (crashed or encountered a fatal bug).
- **Control Icons**: Hover over an agent in the Roster to reveal icons for **Pause**, **Restart**, or **Delete**.

## 5. Interacting with the Grid
Click **GRID** in the top bar to see all active agents in a high-density terminal grid.
- **Direct Input**: Click into any terminal to type directly to that agent.
- **Drag & Drop**: Drag the header of any terminal to reorder your workspace.
- **Focus**: Double-click an agent in the Roster to scroll the grid directly to that terminal.

## Next Steps
- Learn how to manage reusable prompts in the [Library System](./library.md).
- Browse your agent's local files in the [Explorer](./explorer.md).
- Coordinate multi-agent instructions in the [Command Panel](./command-panel.md).
- Manage per-agent Git operations in [Source Control](./source-control.md).
- Configure runtime behavior and shell defaults in [Settings](./settings.md).
- Automate complex tasks with [Visual Workflows](./workflows.md).
