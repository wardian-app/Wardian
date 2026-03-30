# Wardian

<div align="center">

![Wardian Banner](public/icon.png)

**Integrated Agent Environment** — A high-performance habitat for spawning, orchestrating, and monitoring multiple autonomous AI agents.

</div>

---

Named after the Wardian case — the 19th-century terrarium that enabled the global transport of delicate flora — Wardian provides a controlled, persistent environment where autonomous AI agents can operate and collaborate safely. 

Wardian is a governance layer for AI orchestration. It centralizes PTY management, telemetry, and shared context into a unified Command Center, designed for developers who need to manage multiple long-running agent sessions across a single project.

---

## Table of Contents
- [Quick Start](#quick-start)
- [Highlights](#highlights)
- [Core Features](#core-features)
  - [The Command Center](#the-command-center)
  - [Multi-Agent Orchestration](#multi-agent-orchestration)
  - [Library & Skill Management](#library--skill-management)
- [Tech Stack](#tech-stack)
- [Architecture](#architecture)
- [Development Setup](#development-setup)
- [License](#license)

---

## Quick Start

Ensure you have Rust, Node.js (v18+), and a supported provider CLI (e.g. `@google/gemini-cli` or `@anthropic-ai/claude-code`) installed.

```bash
git clone https://github.com/tangemicioglu/Wardian.git
cd Wardian
npm install
npm run dev
```

---

## Highlights

- **Local-first Control Plane**: A single, offline-first dashboard to monitor CPU, memory, and task status across dozens of active AI instances.
- **Unified Terminal Grid**: Instantly switch between high-level metric views and raw, multi-pane PTY shell streams.
- **Dynamic Skill Deployment**: An integrated library manager that rapidly injects or strips filesystem-based capabilities from targeted agent workspaces.
- **Bulk Orchestration**: Broadcast commands, context updates, and strict operational pauses to filtered watchlists simultaneously.

---

## Core Features

### The Command Center
Wardian provides a dense, tactile desktop interface designed for high-bandwidth orchestration.
- **Dual-Sidebar Layout**: The Left Rail houses fast-access controls for Agent Configuration, Command Broadcasting, and Library Management. The Right Sidebar provides a searchable, collapsible agent roster with custom watchlists and drag-and-drop prioritization.
- **Context-Aware Dashboard**: A primary view displaying high-level telemetry (CPU, Memory, Uptime) alongside an action matrix that allows for surgical agent control (Pause, Restart, Query, Delete).
- **Dynamic Terminal Grid**: For deeper debugging, switch to the multi-slot PTY grid to monitor live raw outputs from your agents. Support includes 1x1, 2x2, or focused 1+2 layouts.

### Multi-Agent Orchestration
Scale your workflows by coordinating independent, specialized agents rather than relying on a single monolithic prompt.
- **Persona Class System**: Spawn new agents from pre-configured default classes (e.g., Coder, Architect, Researcher) or define custom personas tailored exactly to your repository's conventions.
- **Broadcast & Bulk Actions**: Dispatch unified instructions, project context, or terminal commands to all agents or a filtered subset simultaneously via the global Command Panel.
- **Real-Time Telemetry**: Wardian actively polls system processes and parses underlying CLI logs to provide accurate activity states (Idle, Processing, Action Needed) and query counts.

### Library & Skill Management
Maintain strict modularity by keeping your prompts and agent capabilities highly organized.
- **Prompt Library**: Store, tag, and manage reusable markdown prompts. These can be assigned as "Quick Prompts" to inject directly into active terminals, eliminating repetitive typing.
- **Skill Deployment**: A physical, filesystem-based skill manager. Rather than relying on fragile global contexts, Wardian can deploy specific capabilities directly to the global user profile, custom classes, or isolated agent environments.

---

## Tech Stack

| Layer       | Technology                                   |
| ----------- | -------------------------------------------- |
| Framework   | [Tauri v2](https://tauri.app/)               |
| Backend     | Rust, `portable-pty` (**ConPTY** on Windows) |
| Frontend    | React 19, TypeScript 5.8, Vite 6             |
| Terminal    | xterm.js 6 + FitAddon                        |
| Styling     | Tailwind CSS v4                              |
| Persistence | `serde_json` (AppData local storage)         |

---

## Architecture

Wardian is built with a focus on modularity, thread safety, and separation of concerns.

### Backend (Rust / Tauri v2)
- **Modular Domain Design**: Specialized modules organized cleanly into `commands`, `models`, `state`, and `utils`.
- **PTY Management**: Leveraging `portable-pty` with native **ConPTY** support ensures robust, true-to-life terminal emulation across operating systems.
- **State Sovereignty**: A centralized `AppState` utilizing async-aware locking (`tokio`) to safely coordinate fast-moving metrics and UI IPC signals.

### Frontend (React 19 / TypeScript)
- **Infrastructure vs. Feature Split**:
    - **Layout**: Persistent structural components (Sidebars, Roster, Titlebars).
    - **Features**: Domain-driven logical boundaries (Agent lifecycle, Terminal implementation).
    - **Views**: Page-level containers for switching display modes (Dashboard, Grid).
- **Type Safety**: Strictly typed interfaces for agent telemetry, system configurations, and data transport models located in `src/types/`.

---

## Development Setup

1. **Rust**: Install [rustup.rs](https://rustup.rs/) (latest stable).
2. **Node.js**: Ensure Node.js (v18+) is installed.
3. **Agent CLIs**: Install supported providers globally (e.g., `npm install -g @google/gemini-cli`) and ensure they are successfully authenticated in your terminal first.
4. **Clone & Install**:
   ```bash
   git clone https://github.com/tangemicioglu/Wardian.git
   cd Wardian
   npm install
   ```

To run the application in development mode with live reloading:
```bash
npm run dev
```

To generate a production-ready release executable for your platform:
```bash
npm run tauri build
```

---

## License

[MIT](LICENSE) — Created by Tan Gemicioglu.