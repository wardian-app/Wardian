# Wardian: Integrated Agent Environment

**A unified, high-performance command suite for autonomous agents.** 

Wardian provides a professional-grade interface for managing, monitoring, and automating agent swarms across local and remote environments.

---

## 🚀 Key Features

- **Unified Command Center**: An Obsidian-style layout with a global Top Bar for centralized navigation and telemetry.
- **Dynamic Terminal Grid**: Monitor multiple agent TUIs simultaneously with responsive grid layouts and drag-and-drop reordering.
- **Library System**: A centralized repository for managing agent Blueprints (Classes), reusable Prompts, and modular Skills.
- **File Explorer**: A context-aware tree view for browsing agent workspaces and global project files.
- **Visual Workflow Builder**: Design and execute complex multi-agent sequences using a pulse-based, deterministic execution engine.
- **Agent Watchlist (Roster)**: Real-time telemetry, status tracking, and "thought" stream monitoring in a high-fidelity roster.

## 🛠️ Get Started

Whether you are a user looking to manage your daily agent operations or a developer looking to extend the ecosystem, we have you covered:

- **[User Guide](./guide/getting-started.md)**: Install Wardian, spawn your first agent, and learn the UI.
- **[Workflows](./workflows/index.md)**: Learn how to build, launch, schedule, assign, and troubleshoot workflows.
- **[Developer Docs](./developer/architecture.md)**: Explore the Rust backend, PTY lifecycle, and state management.
- **[Provider Runtime Notes](./developer/provider-runtimes.md)**: Understand the Gemini, Claude, and Codex runtime differences Wardian has to manage.
- **[Agent Roles](./agents/roles.md)**: Understand the specialized missions of Architect, Coder, and more.

## 🏛️ Architecture & Governance

Wardian is built on a **Modular Domain Design** ensuring a strict separation between the "Physical Layer" (Rust/PTY) and the "Logical Layer" (Workflows/Intent).

Read our **[Specifications (Specs)](./specs/index.md)** to understand the strategic rationale behind every core feature.
