# User Interface Overview

Wardian features a high-fidelity, dual-sidebar interface designed for dense information monitoring and rapid multi-agent orchestration.

## 🧱 The Layout Matrix

### 1. Navigation Rail (Far Left)
The thin vertical icon strip used to switch between primary application modes:
- **Agent Configuration**: Spawn new agents and manage their local settings.
- **Command Center**: Broadcast commands to multiple agents simultaneously.
- **Class Manager**: Define and edit agent prompt classes (Architect, Coder, etc.).
- **Workflows**: Access the visual builder and your saved workflow library.
- **Remote Connections**: Manage SSH hosts for remote agent deployment.
- **Settings**: System-wide preferences and theme engine.

### 2. Content Pane (Left)
A context-aware, collapsible sidebar that displays the specific menu or form for the active Navigation Rail icon.
- *Tip*: Collapse this pane (using the `>` button) to maximize your central workspace while keeping the rail icons accessible.

### 3. Main View (Center)
The primary workspace where you interact with agents. Use the **View Switcher** (top right) to toggle modes:
- **GRID**: The primary terminal workspace. Displays agent TUIs in a responsive grid. Supports drag-and-drop reordering.
- **DASHBOARD**: A summary view of system health, active agent status, and resource usage (CPU/Memory).
- **WORKFLOWS**: The visual canvas for building and executing automated agent sequences.
- **QUEUE / GRAPH / GARDEN**: Placeholder views for future high-fidelity interaction visualizations.

### 4. Agent Watchlist (Far Right)
A collapsible, high-fidelity roster for real-time monitoring of all agent instances.
- **Status Indicators**: Emerald (Idle), Cyan (Processing), Amber (Action Needed), Gray (Off), Red (Error).
- **Thought Bubbles**: Real-time "thought" stream showing what the agent is currently working on.
- **Watchlists**: Organize agents into custom groups (e.g., "Frontend Team", "Critical Tasks") for easier monitoring.

## 🖱️ Interaction Patterns

### Drag and Drop
- **In Grid**: Drag agent headers to reorder them within the grid layout.
- **In Watchlist**: Drag agents to reorder them in the roster or move them between custom watchlists.

### Selection & Multi-Action
- **Single Click**: Focuses an agent in the watchlist and highlights it in the grid.
- **Ctrl/Cmd + Click**: Select multiple agents for broadcast commands or bulk actions.
- **Shift + Click**: Select a range of agents in the watchlist.

### Telemetry & Notifications
- **Global Header**: Real-time aggregate CPU and Memory usage across all active agents.
- **Alerts**: Floating notifications appear in the top right for critical agent feedback or tool requirements.
