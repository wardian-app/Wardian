# Wardian Project Roadmap

Wardian is an advanced Agent Terminal Manager designed for managing multiple autonomous agents through a unified, high-performance interface. This document outlines the development phases for implementing the core features.

## Phase 1: Layout & Terminal Management

\_Goal: Provide a TradingView-like experience for agent orchestration.

- [ ] **Dual-Sidebar "Command Center" Architecture**
  - **Left Sidebar (Primary)**:
    - A thin **Icon Bar** for switching between views (Explorer, Connections, Workflows, Settings).
    - A collapsible **Content Pane** that displays the menu for the active icon (e.g., Spawn Instance form, SSH hosts).
  - **Right Sidebar (Secondary)**:
    - A **collapsible, searchable agent list** (TradingView-style) for rapid selection, status monitoring, and drag-drop reordering.
  - Support for multi-select in the right sidebar and main grid views.
- [ ] **Dynamic Grid Layouts**
  - Implement predefined grid templates (Single, 2x2, 1+2, etc.).
  - Support for drag-and-drop reordering of terminals.
  - Ability to "move" selected agents between grid slots or view layers.
  - Perspective/Layout saving and restoration across sessions.
- [ ] **Dashboard Command Matrix Refresh**
  - Replace generic buttons with standard action set:
    - **Delete**: Terminates the active process and removes it from the roster.
    - **Pause**: Suspends the terminal process but preserves agent session and metadata in `Wardian_state.json`.
    - **Query**: A versatile prompt injection tool (evolution of "Summarize") for rapid context extraction.
    - **Restart**: Resets the PTY session and re-initializes the agent from its current configuration.
- [ ] **Selection-Based Orchestration**
  - Target broadcasting to only selected agents.
  - Bulk actions (terminate, restart, group) for selected instances.
  - "Pinning" agents to specific grid slots from the sidebar list.
- [ ] **Identity & Customization**
  - Support for renaming agents (display names vs. session IDs).
  - Custom color coding and icons for different agent roles.

## Phase 2: Agent Infrastructure & CLI Utility

_Goal: Empower agents with local persistence and self-awareness._

- [ ] **Home Directories**
  - Automate creation of `~/.Wardian/agents/<session_id>/` for each instance.
  - Isolate temporary and permanent files per agent.
- [ ] **Wardian CLI Utility**
  - Implement a lightweight binary/script accessible within the agent's PTY.
  - `Wardian whoami`: Returns current session ID and role.
  - `Wardian notify "message"`: Triggers a UI notification from the agent.
- [ ] **Session Branching (Forking)**
  - Support for "cloning" a session ID to start a new parallel conversation from a snapshot.
- [ ] **Agent-Specific Include Directories**
  - Ability to specify additional `include` paths for each agent to monitor or reference beyond its base `folder`.
- [ ] **CLI Portability Tools**
  - Implement a "Copy Full CLI Command" button in the agent UI.
  - Generates the exact command needed to resume the session manually in an external terminal (e.g., `cd D:\Project && gemini --resume 1a2b3c...`).
- [ ] **Optional Startup Management**
  - Ability to toggle "Spawn at Startup" on a per-agent basis.
  - Agents with startup disabled will appear in the roster but remain in a "Hibernating" state until manually started.
- [ ] **Scheduled Task Engine**
  - Implement a scheduler for recurring agent tasks (e.g., "Every 2 hours, audit the codebase for tech debt").
  - Support for one-off scheduled prompts ("At 5 PM, summarize the day's progress").
  - UI for managing, pausing, and auditing scheduled executions.

## Phase 3: Communication & Orchestration

_Goal: Enable collaborative workflows between isolated agent instances._

- [ ] **Agent-to-Agent IPC & Routing**
  - Implement a message bus (Pub/Sub) in the Rust backend.
  - **Deterministic Routing Engine**: Define UI-based rules for routing JSON outputs between agents.
- [ ] **Human-in-the-Loop (HITL) Queue**
  - Centralized approval UI for sensitive agent actions.
- [ ] **Context Janitor (Memory Management)**
  - Automated summarization workflows to compress context windows when token limits are approached.

## Phase 4: Connectivity & Remote Management

_Goal: Expand the reach of Wardian across environments._

- [ ] **Native SSH & Multiplexing**
  - Support spawning agents on remote hosts via SSH.
  - Integrate with `tmux` for persistent remote sessions that survive disconnection.
- [ ] **Cross-Platform Compatibility**
  - Hardening PTY implementation for Linux and macOS.

## Phase 5: Polish & Ecosystem

- [ ] **High-Fidelity Interaction Visualization**
  - A dynamic, node-link swarm visualization of agents interacting in real-time.
  - Visualizes message passing, task delegation, and collective thought processes.
- [ ] **File-System Watcher Hooks**: Trigger agent tasks based on local file changes.
- [ ] **Theme Engine**: Support for system-wide themes and OLED black mode.
