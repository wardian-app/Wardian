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
- **`send_binary_input_to_agent`**: Routes raw byte sequences from xterm to an agent's `stdin` without UTF-8 re-encoding.

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

### `agent-pty-output-ready`

Pushed when the PTY reader appends new plain terminal output into an agent's buffered `output_buffer`.

```json
{
  "session_id": "uuid-1"
}
```

The event does not carry terminal text directly. The UI should treat it as a readiness signal and immediately drain the buffer via `read_agent_pty` until that command returns `null`.

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

## AgentConfig Provider Config

Provider-specific launch settings are nested under `provider_config` instead of
being authored as new top-level `AgentConfig` fields. The wire shape is an
internally tagged object whose `type` matches the selected top-level
`provider`:

```json
{
  "provider": "codex",
  "provider_config": {
    "type": "codex",
    "sandbox_mode": "workspace-write",
    "approval_policy": "never"
  }
}
```

Shared launch settings such as `model`, `include_directories`,
`system_include_directories`, `custom_args`, `debug`, and
`session_persistence` remain top-level. Legacy flat provider fields may still be
returned while reading older persisted state, but new spawn, clone, and explicit
config update requests should send the nested `provider_config` shape.

## 🛠️ Global Governance Contract

- **Stop All (Safety-First)**: Invokes `stop_all_triggers`. This affects background automation only.
- **Pause/Resume All**: Globally freezes or thaws the evaluation of cron and file-system triggers in the Heartbeat thread without aborting the background tasks themselves.
