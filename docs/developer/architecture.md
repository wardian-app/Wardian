# Wardian Architecture Overview

Wardian is built as a **High-Performance Hybrid Environment**, using **Rust (Tauri)** for the physical/logical system layer and **React (TypeScript)** for the high-fidelity user interface.

## 🏛️ System Layers

### 1. The Physical Layer (Rust Backend)
- **Source of Truth**: The Rust backend is the definitive authority on all agent sessions, PTY states, and telemetry.
- **PTY Management**: Uses `portable-pty` for cross-platform PTY handles. On Windows, it leverages `win32job` to ensure child processes are strictly terminated when the agent session ends.
- **State Management**: `AppState` holds `Mutex`-protected maps of active agents, metrics, and background triggers.
- **Worker Threads**:
    - **Heartbeat (Scheduler)**: Handles periodic tasks and cron triggers.
    - **Metrics Push**: Pushes system/agent resource usage to the UI via Tauri events.

### 2. The Logical Layer (Workflow Engine)
- **Deterministic Execution**: Detailed in [Workflow Engine Architecture](./workflow-engine.md).
- **Shared Registry**: A global Handlebars-based registry where agent outputs are stored for cross-agent referencing.
- **Node Execution**: Deterministic execution of workflow nodes (loops, triggers, agent calls).
- **Injection Logic**: Solves CLI input limits by writing prompts to temp files (`C:\Users\tgemi\.gemini\tmp\wardian-1`) and using `<` redirection.

### 3. The UI Layer (React Frontend)
- **Passive Observation**: The UI primarily observes and edits the state; it does not manage process lifecycles.
- **Visual Builder**: A specialized canvas for designing complex multi-agent workflows, featuring the [Integrated Variable Assistant](./visual-builder.md).
- **Dynamic Grid**: A responsive grid system for monitoring multiple terminal TUIs simultaneously.

## 📡 Communication (IPC)
- **Events (Push)**: Rust pushes telemetry (`agent-metrics`) and logs (`agent-json-event`) to the UI.
- **Commands (Pull)**: The UI invokes Rust functions for high-level actions (`spawn_agent`, `run_workflow`).
- **Terminal Input**: A dedicated `terminal-input` listener in Rust routes keystrokes directly to the corresponding PTY stdin.
