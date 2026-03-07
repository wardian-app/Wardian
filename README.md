# Wardian

**Integrated Agent Environment** — A desktop application for spawning, orchestrating, and monitoring multiple autonomous AI agents through a unified, high-performance interface.

Named after the [Wardian case](https://en.wikipedia.org/wiki/Wardian_case), a sealed glass terrarium that allowed delicate plants to thrive during long voyages — Wardian provides a controlled environment where AI agents can operate, collaborate, and grow.

---

## Features

### Agent Management

- **Spawn multiple agents** with customizable classes (Coder, Reviewer, QA, Architect, Researcher, Designer, and more)
- **Custom agent classes** — define your own roles with tailored system prompts and GEMINI.md configurations
- **Session resumption** — all sessions can be resumed externally via `gemini --resume [session_id]`
- **Real-time telemetry** — track query counts, token usage, and processing status per agent

### TradingView-Style Watchlist Sidebar

- **Tabbed watchlists** — organize agents into multiple named lists with tab pill navigation
- **Multi-list membership** — agents can belong to several watchlists simultaneously
- **Mouse-based drag-and-drop** reordering within lists
- **Right-click context menu** — Rename, Query, Pause, Restart, Delete, Add/Remove from lists
- **Persistent storage** — watchlist configuration saved to AppData

### Terminal Interface

- **Embedded xterm.js terminals** powered by native PTY (ConPTY on Windows)
- **Dashboard and Grid view modes** — switch between compact overview and full terminal access
- **Broadcast commands** to selected agents simultaneously
- **Inline agent renaming** with double-click
- **Status indicators** — Idle (gray), Processing (gold), Action Required (yellow), Error (red)

### Architecture

- **Rust backend** (`src-tauri/`) — session lifecycle, PTY management, telemetry, and state persistence via Tauri v2
- **React + TypeScript frontend** (`src/`) — responsive UI with Tailwind CSS v4
- **Agent class system** — default classes ship with curated system prompts; custom classes stored in AppData
- **State files** — `wardian_state.json` (agent sessions), `watchlists.json` (sidebar lists), `custom_classes.json` (user-defined roles)

---

## Tech Stack

| Layer     | Technology                         |
| --------- | ---------------------------------- |
| Framework | [Tauri v2](https://tauri.app/)     |
| Backend   | Rust, `portable-pty`, `serde_json` |
| Frontend  | React 19, TypeScript 5.8, Vite 6   |
| Terminal  | xterm.js 6 + FitAddon              |
| Styling   | Tailwind CSS v4                    |
| Testing   | Vitest, React Testing Library      |

---

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Node.js](https://nodejs.org/) (v18+)
- [Gemini CLI](https://github.com/google-gemini/gemini-cli) installed and authenticated

### Development

```bash
# Install dependencies
npm install

# Run in development mode
npm run dev

# Run tests
npm test

# Build for production
npm run build
```

### Project Structure

```
wardian/
├── src/                      # React frontend
│   ├── App.tsx               # Main application component
│   ├── AgentWatchlist.tsx     # TradingView-style sidebar
│   ├── watchlistTypes.ts     # Watchlist type definitions
│   ├── watchlistUtils.ts     # Pure utility functions
│   ├── statusUtils.ts        # Agent status derivation
│   └── types.ts              # Core TypeScript interfaces
├── src-tauri/                # Rust backend
│   ├── src/
│   │   ├── lib.rs            # Tauri commands & app setup
│   │   └── manager.rs        # Agent lifecycle & telemetry
│   ├── agent_prompts/        # System prompts per class
│   └── src/default_classes.json
├── ROADMAP.md                # Development roadmap
└── GEMINI.md                 # AI agent guidelines
```

---

## Default Agent Classes

| Class                  | Role                                                      |
| ---------------------- | --------------------------------------------------------- |
| **Coder**              | Writes clean, efficient code from specifications          |
| **Reviewer**           | Audits code for security, performance, and best practices |
| **QA**                 | Writes tests and defines test plans                       |
| **Architect**          | Designs scalable system architectures                     |
| **Coordinator**        | Coordinates and delegates tasks between agents            |
| **Evolver**            | Iteratively optimizes systems and workflows               |
| **Researcher**         | Deep-dives into topics with structured analysis           |
| **Editor**             | Refines written content for clarity and flow              |
| **Personal Assistant** | Manages schedules, drafts, and admin tasks                |
| **Designer**           | Crafts UI/UX designs with focus on aesthetics             |

---

## License

[MIT](LICENSE)
