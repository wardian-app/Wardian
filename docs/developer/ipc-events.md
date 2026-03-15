# IPC and Event Governance

Wardian uses a **Bidirectional Event System** over Tauri's IPC bridge. This document defines the "Contract" between the Rust backend (Source of Truth) and the React frontend (Observer).

## 📡 Commands (Frontend to Backend)

The UI invokes these functions via `invoke("command_name", { args })`.

### Agent Lifecycle

- **`list_agents`**: Returns a list of all active/hibernating agent configurations.
- **`spawn_agent`**: Spawns a new PTY session from an `AgentConfig`.
- **`kill_agent`**: Terminates the PTY process and removes it from state.
- **`pause_agent`**: Suspends the PTY (killing the process but keeping the config).
- **`resume_agent`**: Restarts a paused PTY session.
- **`send_input_to_agent`**: Routes raw keystrokes to an agent's `stdin`.

### Workflow Governance

- **`run_workflow`**: Initiates a one-off workflow run.
- **`stop_all_triggers`**: Aborts all active background workflow tasks (Stop All).
- **`pause_all_triggers`**: Signals the Heartbeat to temporarily skip trigger evaluation.
- **`resume_all_triggers`**: Resumes trigger evaluation in the Heartbeat thread.

## 🔔 Events (Backend to Frontend)

The UI listens for these events using `listen("event-name", (event) => { ... })`.

### `agent-metrics`

Pushed every 5 seconds. Provides aggregate resource usage.

```json
[
  {
    "session_id": "uuid-1",
    "cpu_usage": 12.5,
    "memory_mb": 450,
    "query_count": 5
  }
]
```

### `agent-json-event`

Pushed whenever an agent's PTY output contains valid JSON (e.g., from Gemini CLI).

- **`type: "progress"`**: Used for the "Thought Stream" bubbles.
- **`type: "alert"`**: Triggers floating UI notifications.
- **`type: "info" | "model"`**: Signals the agent has finished a turn (Idle state).

### `workflow-telemetry`

Pushed in real-time as the Workflow Engine executes nodes.

```json
{
  "workflow_id": "uuid-a",
  "node_id": "node-1",
  "status": "processing | success | error",
  "output": { "text": "..." },
  "error": "..."
}
```

### `agents-updated`

Emitted whenever the global agent roster changes (spawned, renamed, killed). Signals the UI to refresh its list via `list_agents`.

## 🛠️ Global Governance Contract

- **Stop All (Safety-First)**: Invokes `stop_all_triggers`. This affects background automation only.
- **Pause/Resume All**: Globally freezes or thaws the evaluation of cron and file-system triggers in the Heartbeat thread without aborting the background tasks themselves.
