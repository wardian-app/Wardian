# Wardian

<div align="center">

<img src="public/icon.png" width="128" alt="Wardian Logo" />

**A modular, local-first desktop habitat for agents, workflows, and reusable context.**

[![tests](https://github.com/wardian-app/Wardian/actions/workflows/ci.yml/badge.svg)](https://github.com/wardian-app/Wardian/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/wardian-app/Wardian/branch/main/graph/badge.svg)](https://codecov.io/gh/wardian-app/Wardian)
[![Release](https://img.shields.io/github/v/release/wardian-app/Wardian?label=release)](https://github.com/wardian-app/Wardian/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/wardian-app/Wardian/total?label=downloads)](https://github.com/wardian-app/Wardian/releases)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-555)

[![Wardian Demo](public/demo.gif)](public/demo.gif)

</div>

---

> 🚧 **Early development.** Wardian is under active construction. Expect rough edges: APIs, on-disk formats, and UI layouts can change between releases without notice. Pin a version if you depend on it, and please [file an issue](https://github.com/wardian-app/Wardian/issues) when something breaks.

Wardian gives the agent tools you already run durable identity, live terminals, scoped skills, workflow runs, queue evidence, and workspace context in one GUI-first habitat.

Use it to spawn specialized agents, monitor their progress, hand work between them, collect completed output, and automate repeatable flows across providers such as Antigravity, Claude, Codex, and OpenCode. The bundled `wardian` CLI gives agents and scripts a textual control surface for discovering identity, coordinating peers, and controlling Wardian without driving the graphical app.

Wardian is built for malleable agent work. Prompts, classes, skills, workflows, queues, and memory-ready evidence are treated as visible, reusable artifacts rather than opaque app state. Its tab-based Workbench lets surfaces move between panes, split side by side, and restore across launches while the agent roster and auxiliary tools remain available. You can start by watching live agents, then gradually turn repeated instructions into prompts, reusable roles, deployable skills, workflow templates, and durable project context.

---

## Install

Use the supported install path for your platform:

| System | Install |
| :--- | :--- |
| Windows x64 | `winget install WardianApp.Wardian` |
| macOS Apple Silicon or Intel | `brew install --cask wardian-app/tap/wardian` |
| Linux Debian/Ubuntu x64 | Download `Wardian_X.Y.Z_amd64.deb` from [Releases](https://github.com/wardian-app/Wardian/releases/latest), then run `sudo apt install ./Wardian_X.Y.Z_amd64.deb`. |
| Linux other x64 | Download `Wardian_X.Y.Z_amd64.AppImage` from [Releases](https://github.com/wardian-app/Wardian/releases/latest), then run `chmod +x Wardian_X.Y.Z_amd64.AppImage && ./Wardian_X.Y.Z_amd64.AppImage`. |

Manual downloads are also available from the [Releases page](https://github.com/wardian-app/Wardian/releases).
Choose the asset for your operating system and CPU:

| System | Download asset | Notes |
| :--- | :--- | :--- |
| Windows x64 | `Wardian_X.Y.Z_x64-setup.exe` | Standard Windows installer. |
| macOS Apple Silicon | `Wardian_X.Y.Z_aarch64.dmg` | For M-series Macs such as M1, M2, M3, or M4. |
| macOS Intel | `Wardian_X.Y.Z_x64.dmg` | For older Intel Macs. |
| Linux Debian/Ubuntu x64 | `Wardian_X.Y.Z_amd64.deb` | Installable Debian package. |
| Linux other x64 | `Wardian_X.Y.Z_amd64.AppImage` | Portable Linux app. |

`x64` and `amd64` both mean 64-bit Intel/AMD CPUs. On macOS, Apple Silicon uses `aarch64`, not `x64`. Ignore updater-only assets such as `latest.json`, `.app.tar.gz`, or `.sig` files when installing manually.

Debian/Ubuntu users who want package-manager updates can use the optional
[Wardian APT repository](docs/developer/package-manager-distribution.md#linux-apt-repository).

> **Note:** Wardian binaries are currently unsigned. On first launch:
> - **Windows:** SmartScreen will show a warning. Click "More info" → "Run anyway."
> - **macOS:** Gatekeeper will refuse to open the app. Right-click the app and choose "Open," or run `xattr -cr /Applications/Wardian.app` from Terminal.
> - **Linux:** APT installs update through the system package manager. `.AppImage` is portable (`chmod +x` and run); direct `.deb` downloads install via `sudo apt install ./Wardian_*.deb`.

---

## Table of Contents

- [Quick Start](#quick-start)
- [Documentation](#documentation)
- [Supported Providers](#supported-providers)
- [Why Wardian?](#why-wardian)
- [Core Features](#core-features)
- [Platform Support](#platform-support)
- [Product Direction](#product-direction)
- [Tech Stack](#tech-stack)
- [Architecture](#architecture)
- [Development Setup](#development-setup)
- [License](#license)

---

## Quick Start

New users should start with the public first-run guide:

- [First-Time Install and First Run](https://docs.wardian.org/guide/getting-started)

That guide covers download, launch, provider CLI setup, authentication, the first agent spawn, and Queue review.

For local development, clone the repository and run the dev app:

```bash
git clone https://github.com/wardian-app/Wardian.git
cd Wardian
npm install
npm run dev
```

---

## Documentation

For complete user and developer docs, start here:

- [Public Documentation](https://docs.wardian.org/)
- [User Guide Index](https://docs.wardian.org/guide/)
- [Workbench](https://docs.wardian.org/guide/workbench)
- [Agents Overview](https://docs.wardian.org/guide/agents-overview)
- [Wardian CLI](https://docs.wardian.org/guide/cli)
- [Queue](https://docs.wardian.org/guide/queue)
- [Workflow Reference](https://docs.wardian.org/workflows/)
- [Developer Index](https://docs.wardian.org/developer/)

---

## Supported Providers

Wardian supports five provider CLIs today and adapts each runtime into the same agent lifecycle, telemetry, skill, and workflow model.

| Provider        | Support       | Runtime Model                                              |
| :-------------- | :------------ | :--------------------------------------------------------- |
| **[Antigravity](https://www.antigravity.google/docs/cli-overview)** | ✅ Supported | Real-workspace runtime with native `AGENTS.md` discovery, `agy` conversations, and transcript-based turn detection. |
| **[Claude Code](https://github.com/anthropics/claude-code)** | ✅ Supported  | Real-workspace runtime with explicit session IDs and permission hooks. |
| **[Codex](https://github.com/openai/codex)**       | ✅ Supported  | Real-workspace execution via `--cd` with per-agent `CODEX_HOME` habitat state. |
| **[OpenCode](https://github.com/anomalyco/opencode)**    | ✅ Supported  | Real-workspace runtime with native `AGENTS.md` discovery and injected config for Wardian scope. |
| **[Gemini CLI](https://github.com/google-gemini/gemini-cli)**  | ⚠️ Unmaintained | Real-workspace runtime with patched skill discovery. Consumer/free access ended June 18, 2026. Use Antigravity for Google-model access. |

> See [Provider Runtime Notes](docs/providers.md) for a deep dive into provider-specific discovery and lifecycle management.

---

## Why Wardian?

Most agent tools still live primarily in terminals, config files, or headless
framework code. Wardian is built around a different center of gravity: a
local-first Habitat where humans can see, steer, and reshape many real local
CLI agents without losing the terminal truth underneath.

- **GUI-first, terminal-real.** Wardian gives each managed provider a real PTY
  while projecting status, telemetry, output, queue evidence, workflows, and
  source control into a desktop interface.
- **Persistent agent habitat.** Agents have durable roster identity, class,
  provider, workspace, scoped skills, worktree state, and completion history
  instead of being disposable terminal tabs.
- **Coordinate without babysitting terminals.** Send prompts, structured asks,
  broadcasts, lifecycle commands, and workflow runs through shared app and CLI
  control surfaces.
- **Keep finished work visible.** Queue and workflow evidence preserve completed
  agent output so results do not disappear into scrollback.
- **Turn repetition into reusable capability.** Save prompts, tune classes,
  deploy skills, schedule workflows, and promote useful evidence into durable
  context over time.
- **Stay local and inspectable.** Wardian adapts real CLI providers, real
  workspaces, filesystem-backed libraries, and local workflow records instead of
  hiding orchestration behind a remote black box.

> Explore our [Key Features guide](docs/features.md) for more technical comparisons.

---

## Core Features

### Live Agent Operations

- Spawn and supervise provider CLI agents from one desktop app.
- Keep each agent in a real PTY-backed terminal with live status, telemetry,
  process control, and retained output.
- Arrange agent sessions, Agents Overview, Dashboard, Graph, Queue, Library,
  Garden, and Workflows as tabs or side-by-side Workbench panes.
- Switch Agents Overview between adaptive Auto, multi-card Grid, and focused
  Single layouts without treating Grid as a global app page.

### Coordination and Control

- Send prompts, broadcasts, lifecycle commands, and structured peer handoffs to
  selected agents.
- Use teams, watchlists, and workspaces to track project/workstream context
  without assuming one folder equals one project.
- Let agents and scripts coordinate through the bundled `wardian` CLI: inspect
  rosters, send work, wait for status changes, watch output, and run workflows.

### Reusable Context and Capabilities

- Define class blueprints for repeatable roles such as Coder, Reviewer,
  Architect, Researcher, or project-specific specialists.
- Save reusable prompts and deploy skills globally, by class, or to a single
  agent instance.
- Keep prompts, skills, classes, and provider-specific habitat material
  inspectable and scoped instead of burying them in one monolithic prompt.

### Workflows and Durable Evidence

- Build local workflow templates with agent nodes, branch/loop/wait control,
  shared storage, manual runs, schedules, and listener-style triggers.
- Run workflows through the Rust engine while preserving run state, node output,
  and completion evidence.
- Review agent completions and workflow outcomes in Queue before turning useful
  results into reusable prompts, skills, workflows, or durable memory context.

### Local Project Control

- Work across multiple providers and multiple workspaces without leaving the
  local app.
- Assign Wardian-managed Git worktrees to agents for branch isolation.
- Keep Explorer and Source Control auxiliary panes beside the Workbench to
  inspect files, diffs, commits, and workspace state around the agents doing
  the work.

---

## Platform Support

Wardian leverages native OS capabilities for high-performance terminal emulation.

| OS          | Level     | Backend Implementation                                |
| :---------- | :-------- | :---------------------------------------------------- |
| **Windows** | 🏆 Native | Full **ConPTY** integration via `portable-pty`.       |
| **macOS**   | ✅ Stable | Standard Unix PTY via `portable-pty`.                 |
| **Linux**   | ✅ Stable | Standard Unix PTY via `portable-pty`.                 |

> Detailed platform-specific notes and troubleshooting can be found in [OS Support](docs/os-support.md).

---

## Product Direction

Wardian is evolving toward a malleable home for your agents: a local environment where agent capabilities, workflows, evidence, and project context can be inspected, rearranged, and extended over time.

- **Malleable Workbench**: extend the canonical tab, split, restore, and surface contribution model with new tools such as file editing and browsing.
- **Runtime reliability and provider fidelity**: keep real PTY behavior, provider-specific delivery, transcript capture, and cross-platform process supervision stable as provider CLIs change.
- **Reusable context and capabilities**: continue tightening the Library around prompts, skills, classes, workflow blueprints, MCP configuration, and inspectable filesystem-backed artifacts.
- **Coordination surfaces**: improve Graph topology, teams, watchlists, Queue evidence, structured asks/replies, and CLI automation so multi-agent work stays visible and bounded.
- **Workflow operations**: harden workflow authoring, scheduling, run observation, history, and failure evidence for repeatable local automation.
- **Remote and distribution polish**: improve the mobile remote surface, installer/update paths, package-manager channels, and first-run documentation.

Full details available in [ROADMAP.md](ROADMAP.md).

---

## Tech Stack

| Layer       | Technology                                   |
| ----------- | -------------------------------------------- |
| Framework   | [Tauri v2](https://tauri.app/)               |
| Backend     | Rust, `portable-pty` (**ConPTY** on Windows) |
| Frontend    | React 19, TypeScript 5.8, Vite 6             |
| CLI         | Rust, `clap`, shared `wardian-core` state    |
| Terminal    | xterm.js 6 + FitAddon                        |
| Styling     | Tailwind CSS v4                              |
| Persistence | SQLite `state.db` and JSON app settings      |

---

## Architecture

Wardian is built with a focus on modularity, thread safety, and separation of concerns.

### Backend (Rust / Tauri v2)

- **Modular Domain Design**: Specialized modules organized cleanly into `commands`, `models`, `state`, and `utils`.
- **PTY Management**: Leveraging `portable-pty` with native **ConPTY** support ensures robust, true-to-life terminal emulation across operating systems.
- **State Sovereignty**: A centralized `AppState` utilizing async-aware locking (`tokio`) to safely coordinate fast-moving metrics and UI IPC signals.

### Frontend (React 19 / TypeScript)

- **Infrastructure vs. Feature Split**:
  - **Layout**: persistent structural components such as the Workbench adapter, sidebars, roster, and title bar.
  - **Features**: domain-driven logical boundaries such as Workbench navigation, agent lifecycle, and terminal presentation brokering.
  - **Views and surfaces**: feature containers hosted as Workbench tabs; Agents Overview owns Auto, Grid, and Single display modes.
- **Type Safety**: Strictly typed interfaces for agent telemetry, system configurations, and data transport models located in `src/types/`.

---

## Development Setup

1. **Rust**: Install [rustup.rs](https://rustup.rs/) (latest stable).
2. **Node.js**: Ensure Node.js (v18+) is installed.
3. **Agent CLIs**: Install at least one supported provider CLI before spawning agents: [Antigravity](https://www.antigravity.google/docs/cli-overview) (`agy`), [Claude Code](https://github.com/anthropics/claude-code) (`@anthropic-ai/claude-code`), [Codex](https://github.com/openai/codex) (`@openai/codex`), or [OpenCode](https://github.com/anomalyco/opencode) (`opencode` command, commonly installed from `opencode-ai`). Ensure each provider is authenticated successfully in your terminal first.
4. **Clone & Install**:
   ```bash
   git clone https://github.com/wardian-app/Wardian.git
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
