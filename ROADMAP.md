# Wardian Project Roadmap

Wardian is an advanced Agent Terminal Manager designed for managing multiple autonomous agents through a unified, high-performance interface. This document outlines the development phases for implementing the core features.

## Phase 1: Layout & Terminal Management

_Goal: Provide a professional-grade command center for agent orchestration.

- [x] **Dual-Sidebar "Command Center" Architecture**
  - **Left Sidebar (Primary)**:
    - A thin **Icon Bar** for switching between views (Explorer, Connections, Workflows, Settings).
    - A collapsible **Content Pane** that displays the menu for the active icon (e.g., Spawn Instance form, SSH hosts).
  - **Right Sidebar (Secondary)**:
    - A **collapsible, searchable agent list** for rapid selection, status monitoring, and drag-drop reordering.
  - Support for multi-select in the right sidebar and main grid views.
- [x] **Dynamic Grid Layouts**
  - Implement predefined grid templates (Single, 2x2, 1+2, etc.).
  - Support for drag-and-drop reordering of terminals.
  - Ability to "move" selected agents between grid slots or view layers.
  - Perspective/Layout saving and restoration across sessions.
- [x] **Dashboard Command Matrix Refresh**
  - Replace generic buttons with standard action set:
    - **Delete**: Terminates the active process and removes it from the roster.
    - **Pause**: Suspends the terminal process but preserves agent session and metadata in `Wardian_state.json`.
    - **Query**: A versatile prompt injection tool (evolution of "Summarize") for rapid context extraction.
    - **Restart**: Resets the PTY session and re-initializes the agent from its current configuration.
- [x] **Selection-Based Orchestration**
  - Target broadcasting to only selected agents.
  - Bulk actions (terminate, restart, group) for selected instances.
  - "Pinning" agents to specific grid slots from the sidebar list.
- [x] **Identity & Customization**
  - Support for renaming agents (display names vs. session IDs).
  - Custom color coding and icons for different agent roles.

## Phase 2: Agent Infrastructure & CLI Utility

_Goal: Empower agents with local persistence and self-awareness._

- [x] **Home Directories**
  - Automate creation of per-agent state under the Wardian home for each instance.
  - Isolate temporary and permanent files per agent.
- [x] **Wardian CLI Utility**
  - Implement a lightweight binary accessible from terminals and managed agent processes.
  - `wardian agent`: Returns the current managed session when `WARDIAN_SESSION_ID` is set.
  - `wardian agent list --scope all`: Lists live or persisted agents for coordination.
  - `wardian send`, `wardian ask`, `wardian agent wait`, and `wardian agent watch`: Provide terminal-native coordination and response evidence.
  - `wardian workflow list/show/run/stop`: Expose workflow inspection and live run control from the shell.
- [x] **Session Branching (Forking)**
  - Support cloning an agent configuration into a new parallel session.
- [x] **Agent-Specific Include Directories**
  - Ability to specify additional `include` paths for each agent to monitor or reference beyond its base `folder`.
- [x] **CLI Portability Tools**
  - Expose agent identity, workspace, provider, status, and worktree state through scriptable CLI commands.
  - Keep direct provider resume commands as provider-runtime implementation details rather than the primary user contract.
- [x] **Session Lifecycle Management**
  - Preserve inactive agents in the roster and allow users to start, pause, resume, kill, clone, and reassign sessions explicitly.
- [x] **Scheduled Task Engine**
  - Implement scheduled workflow triggers and scheduled run instances.
  - Support interval, daily, weekly, and one-time scheduled workflow launches.
  - Provide UI and backend commands for managing, pausing, running, and deleting scheduled executions.

## Phase 3: Communication & Orchestration

_Goal: Enable collaborative workflows between isolated agent instances._

- [ ] **Agent-to-Agent IPC & Routing**
  - Implement a message bus (Pub/Sub) in the Rust backend.
  - **Deterministic Routing Engine**: Define UI-based rules for routing JSON outputs between agents.
- [ ] **Human-in-the-Loop (HITL) Queue**
  - Expand the current completion Queue into a centralized approval and interruption surface for sensitive agent actions.
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
- [ ] **Theme Engine**: Support for system-wide themes, OLED black mode, and a functional **Light Mode** toggle.
