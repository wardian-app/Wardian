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

### Queue and Readiness

- **`load_queue_items` / `save_queue_items`**: Load or persist the Queue projection for the active Wardian home. Queue item identity should be derived from canonical evidence IDs rather than frontend timestamps.
- **`load_queue_preferences` / `save_queue_preferences`**: Load or persist per-event-type Queue visibility, desktop alert, and sound alert preferences.
- **`list_provider_readiness`**: Returns install/auth readiness for provider commands before spawn. This is separate from live provider input readiness, which is tracked per agent runtime generation.

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

### `agent-status-updated`

Pushed when the backend accepts a provider runtime status transition for an agent.

```json
{
  "session_id": "uuid-1",
  "current_status": "action_required"
}
```

Provider runtime status is authoritative for provider-internal states such as `idle`, `processing`, `action_required`, `off`, and `error`. Wardian interaction status must not replace provider status. For example, a Codex permission prompt is provider runtime evidence; a queued Wardian ask waiting for delivery remains an interaction delivery state.

### Interaction and Delivery Watch Events

The CLI live-control `agent_watch`, `ask`, `send_message`, and `submit_reply` paths expose ordered watch events through `WatchEvent` records. These records are not raw terminal output.

```json
{
  "cursor": "uuid-1:0000000000000042",
  "kind": "delivery",
  "payload": {
    "uuid": "uuid-1",
    "provider": "codex",
    "runtime_state": "provider_input_ready",
    "delivery_state": "submit_sent_unverified",
    "input_mode": "message",
    "delivery_phase": "submit_key_sent"
  }
}
```

Delivery events describe the transport attempt. Interaction records describe the Wardian-owned communication lifecycle:

```json
{
  "id": "ask_0123456789abcdef",
  "kind": "task",
  "sender_session_id": "source-uuid",
  "target_session_ids": ["target-uuid"],
  "status": "awaiting_reply",
  "trigger_policy": "reply_required",
  "body_ref": {
    "storage": "file",
    "path": "<wardian-home>/agents/target-uuid/mailbox/ask_0123456789abcdef.md"
  },
  "created_at": "2026-05-25T16:00:00.000Z",
  "updated_at": "2026-05-25T16:00:00.000Z"
}
```

Valid interaction kinds are `message`, `task`, `reply`, and `notification`. Valid interaction statuses are `created`, `queued`, `delivering`, `delivered`, `awaiting_reply`, `completed`, `failed`, and `expired`.

Structured replies attach to the parent task interaction and carry the reply status separately:

```json
{
  "request_id": "ask_0123456789abcdef",
  "status": "done",
  "body": "Reviewed the patch. No blocking findings.",
  "target_session_id": "target-uuid",
  "source_session_id": "target-uuid",
  "replied_at": "2026-05-25T16:03:00.000Z"
}
```

### Provider Input State

Live delivery readiness is tracked per provider runtime generation. The generation increments when a provider process is spawned, resumed, cleared, or reattached. Delivery may drain queued work only after readiness evidence for the current generation.

```json
{
  "session_id": "uuid-1",
  "generation": 7,
  "state": "ready",
  "ready_evidence": "prompt_detected",
  "observed_at": "2026-05-25T16:00:00.000Z"
}
```

Valid states are `unknown`, `booting`, `ready`, `busy`, `action_required`, and `unavailable`. Valid readiness evidence values are `provider_event`, `prompt_detected`, `title_detected`, and `manual_status`. Stale readiness from an older generation must be ignored.

### Queue Evidence

Queue items project canonical live evidence instead of replayed terminal text. Queue records can include:

```json
{
  "id": "queue_0123456789abcdef",
  "type": "action_needed",
  "timestamp": 1779724800000,
  "read": false,
  "evidence_id": "provider-event-uuid-1-7-permission-1",
  "evidence_source": "provider_runtime",
  "agent_session_id": "uuid-1",
  "agent_name": "reviewer-a1",
  "summary": "Codex is asking for command approval."
}
```

`evidence_source` is one of `provider_runtime`, `interaction_store`, or `live_runtime`. Hydration may restore existing queue items, but it must not emit new evidence. Replay of provider logs or terminal buffers must not create duplicate Queue cards.

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
