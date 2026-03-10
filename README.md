# Wardian

<div align="center">

![Wardian Banner](public/icon.png)

**Integrated Agent Environment** — A high-performance habitat for spawning, orchestrating, and monitoring multiple autonomous AI agents.

</div>

---

## 🏛️ Purpose & Philosophy

Named after the [Wardian case](https://en.wikipedia.org/wiki/Wardian_case) — the 19th-century terrarium that enabled the global transport of delicate flora — **Wardian** provides a controlled, persistent environment where autonomous AI agents can operate, collaborate, and grow.

Wardian is a **governance layer for AI orchestration**. It centralizes PTY management, telemetry, and shared context into a unified "Command Center," designed for developers who need to manage multiple long-running agent sessions across a single project.

---

## 🚀 Core Features

### 🎮 The Command Center (UI)

- **Dual-Sidebar "TradingView" Experience**:
  - **Left Rail**: Fast-access icons for Agent Configuration, Workflow Builder, and Global Settings.
  - **Right Sidebar**: A searchable, collapsible agent roster with tabbed watchlists and drag-and-drop reordering for rapid selection.
- **Context-Aware Dashboard**: High-level telemetry (CPU/Memory) alongside a 2x2 Action Matrix (Delete, Pause, Query, Restart) for surgical agent control.
- **Dynamic Terminal Grid**: Switch between Dashboard overview and a multi-slot PTY grid to monitor live agent output across 1x1, 2x2, or 1+2 layouts.

### 🤖 Multi-Agent Orchestration

- **Persona Class System**: Spawn agents from pre-configured classes (Coder, Architect, Researcher) or define custom personas with tailored system prompts.
- **Broadcast & Bulk Actions**: Dispatch unified instructions to all agents or a filtered subset simultaneously.
- **Manual Resumption**: Every agent session is uniquely identified and can be resumed externally via `gemini --resume [session_id]`.

### 🛠️ Agent-to-System Integration

- **Wardian CLI**: A lightweight binary (`wardian`) injected into every agent's PTY, providing agents with "self-awareness" (identity, logs, workspace info) and the ability to trigger system notifications.
- **Shared Storage**: A persistent, key-value store accessible by all agents in a workspace for cross-session state sharing.

---

## 📖 How to Use

1.  **Spawn an Agent**: Open the **Left Sidebar** and click the **Agent Configuration** icon. Fill out the "Spawn Instance" form (e.g., Name: `Refactor_Bot`, Class: `Coder`).
2.  **Monitor Status**: Check the **Right Sidebar** (Roster). The status light indicates if the agent is **Idle** (Emerald), **Processing** (Cyan), or requires **Action** (Amber).
3.  **Broadcast Commands**: Use the **Command** tab in the Left Sidebar to send a single instruction (e.g., `git status`) to all selected agents at once.
4.  **Manage Life-cycle**: Use the **Dashboard View** (Main Stage) to quickly Pause, Restart, or Delete specific agents using the action grid.
5.  **Resume Anywhere**: Copy the session ID from the agent's identity info to resume the conversation in any external terminal using `gemini --resume <id>`.

---

## 🗺️ Roadmap

- **JSON-Native Workflow Engine**: Build multi-step, autonomous agent sequences using a visual, node-based builder.
- **Human-in-the-Loop (HITL) Queue**: A centralized approval UI for agent actions that require explicit human permission.
- **Garden View & Swarm Topology**: Advanced data visualizations representing agent health, semantic clustering, and real-time interaction meshes.
- **Native SSH & Multiplexing**: Extend orchestration to remote hosts with persistent `tmux` integration.

---

## 💻 Tech Stack

| Layer       | Technology                                   |
| ----------- | -------------------------------------------- |
| Framework   | [Tauri v2](https://tauri.app/)               |
| Backend     | Rust, `portable-pty` (**ConPTY** on Windows) |
| Frontend    | React 19, TypeScript 5.8, Vite 6             |
| Terminal    | xterm.js 6 + FitAddon                        |
| Styling     | Tailwind CSS v4                              |
| Persistence | `serde_json` (AppData local storage)        |

---

## 🏛️ Architecture

Wardian is built with a focus on modularity, thread safety, and separation of concerns.

### Backend (Rust / Tauri v2)
- **Modular Domain Design**: Specialized modules for `commands`, `models`, `state`, and `utils`.
- **PTY Management**: Leveraging `portable-pty` with native **ConPTY** support for robust terminal emulation on Windows.
- **State Sovereignty**: A centralized `AppState` using async-aware locking (`tokio`) to coordinate metrics and IPC.

### Frontend (React 19 / TypeScript)
- **Infrastructure vs. Feature Split**:
    - **Layout**: Persistent structural components (e.g., Sidebars, Roster).
    - **Features**: Domain-driven logic (e.g., Agent lifecycle, Terminal implementation).
    - **Views**: Page-level containers for different display modes (Dashboard, Grid).
- **Type Safety**: Strictly typed interfaces for agent telemetry and configurations located in `src/types/`.

### 📂 Data Persistence Strategy

Wardian maintains a modular state architecture to ensure performance and portability:

- `wardian_state.json`: Active session lifecycle and PTY telemetry.
- `watchlists.json`: Sidebar organization and custom agent groups.
- `custom_classes.json`: User-defined persona prompts and configurations.
- `workflows.json`: The central registry for all saved automation graphs and flows.


---

## 🛠️ Getting Started

### 1. Dev Environment Setup

1.  **Rust**: Install [rustup.rs](https://rustup.rs/) (latest stable).
2.  **Node.js**: Ensure Node.js (v18+) is installed.
3.  **Gemini CLI**: `npm install -g @google/gemini-cli` (Authenticated).
4.  **Clone & Install**:
    ```bash
    git clone https://github.com/tangemicioglu/Wardian.git
    cd Wardian
    npm install
    ```

### 2. Development Workflow

To run the application in development mode with live reloading:

```bash
npm run dev
```

### 3. Production Build

To generate a production-ready release executable for your platform:

```bash
npm run tauri build
```

---

## 📄 License

[MIT](LICENSE) — Created by [Tan Gemicioglu](https://github.com/tangemicioglu).
