# Wardian Architecture Overview

Wardian is built as a **High-Performance Hybrid Environment**, using **Rust (Tauri)** for the physical/logical system layer and **React (TypeScript)** for the high-fidelity user interface.

## 🏛️ System Layers

### 1. The Physical Layer (Rust Backend)

- **Source of Truth**: The Rust backend is the definitive authority on all agent sessions, PTY states, and telemetry.
- **Shared Core**: `crates/wardian-core` owns shared paths, SQLite migrations, agent DTOs, and identity lookup so the Tauri app and CLI use the same durable state contract.
- **PTY Management**: Uses `portable-pty` for cross-platform PTY handles. On Windows, it leverages `win32job` to ensure child processes are strictly terminated when the agent session ends.
- **Provider Adapters**: Agent CLIs are integrated behind a Rust provider layer so session spawn, headless execution, and telemetry enrichment can support Gemini, Antigravity, Claude, Codex, and OpenCode without rewriting the rest of the backend.
  See [Provider Runtime Notes](./provider-runtimes.md) for the provider-specific working-root, skill, and session rules that sit behind this abstraction.
- **Habitat Projection**: For providers that cannot natively discover Wardian instructions and skills from external include roots, the backend materializes a neutral per-session `habitat` directory. That habitat links the real workspace, projects a scoped `AGENTS.md`, and exposes provider-native skill layouts without mutating the user repository. OpenCode is an explicit exception: it stays in the real workspace and receives class/skill scope through injected runtime config instead of a projected workspace.
- **State Management**: `AppState` holds `Mutex`-protected maps of active agents, metrics, workflow runs, and background tasks.
- **Worker Threads**:
  - **Workflow Scheduler**: Fires persisted workflow schedule invokers.
  - **Metrics Push**: Pushes system/agent resource usage to the UI via Tauri events.
- **App Queue Persistence**: Completion triage state is stored under the active Wardian home so agent and workflow outcomes survive app restarts.

### 2. The Logical Layer (Workflow Engine)

- **Deterministic Execution**: Detailed in [Workflow Engine Architecture](./workflow-engine.md).
- **Shared Registry**: A global Handlebars-based registry where agent outputs are stored for cross-agent referencing.
- **Workflow Candidate Queue**: Deterministic execution of workflow nodes (loops, triggers, waits, branches, memory, commands, and agent calls) through the engine's internal candidate-node FIFO.
- **Injection Logic**: Solves CLI input limits by writing prompts to temp files (`~\.gemini\tmp\wardian-1`) and using `<` redirection.

### 2.5 Memory and Knowledge

- **Continuity vs Memory**: Provider-native resume state is not the same thing as long-term memory. Session IDs, PTY ownership, approval hooks, and provider trust remain runtime concerns.
- **Evidence-First Memory**: Wardian's memory direction is to preserve raw evidence, index it for retrieval, and build prompt context selectively rather than replaying prior sessions wholesale.
- **Promoted Knowledge**: Curated atoms remain useful, but as promoted knowledge with provenance back to retrieved evidence rather than as the primary memory substrate.
- The internal evidence-first memory spec records the design history for this direction.

### 3. The UI Layer (React Frontend)

- **Passive Observation**: The UI primarily observes and edits the state; it does not manage process lifecycles.
- **Visual Builder**: A specialized canvas for designing complex multi-agent workflows, featuring the [Integrated Variable Assistant](./visual-builder.md).
- **Dynamic Grid**: A responsive grid system for monitoring multiple terminal TUIs simultaneously.
- **Queue View**: A triage surface for unread agent completions and workflow outcomes.

## 📡 Communication (IPC)

Wardian uses a bidirectional event system, detailed in [IPC and Event Governance](./ipc-events.md).

- **Events (Push)**: Rust pushes telemetry (`agent-metrics`), structured logs (`agent-json-event`), and PTY readiness notifications (`agent-pty-output-ready`) to the UI.
- **Commands (Pull)**: The UI invokes Rust functions for high-level actions (`spawn_agent`, `workflow_run`).
- **Terminal Input**: The UI invokes `send_input_to_agent` and `send_binary_input_to_agent` directly so PTY control replies and raw mouse bytes take the shortest path back to the agent process.
- **Terminal Host Lifecycle**: The frontend keeps one live xterm instance per session and reattaches its DOM host across pane remounts instead of disposing and reconstructing the emulator.

## Wardian CLI

The `crates/wardian-cli` binary shares DTOs, paths, migrations, identity filters, and the live control protocol through `wardian-core`. Wardian remains GUI/app-first; the CLI exists so agents and automation can inspect and control Wardian through a stable textual surface. For read commands it first tries the running desktop app's local control endpoint for the same `WARDIAN_HOME` and falls back to `$WARDIAN_HOME/state.db` when the app is not running. Live-control commands cover agent lifecycle, message delivery, watch/wait coordination, worktree assignment, and workflow run control. The desktop app stages the binary as a Tauri resource and installs it into the user Wardian bin directory on startup.
