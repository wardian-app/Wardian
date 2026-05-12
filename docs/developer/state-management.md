# State Management in Wardian

Wardian uses a **Centralized, Thread-Safe Registry** model in the Rust backend to manage the complex lifecycles of multiple autonomous agents.

## 🏛️ AppState (The Global Registry)
Located in `src-tauri/src/state/app_state.rs`, the `AppState` is managed as a Tauri State (`tauri::State<AppState>`).

### Key Fields:
- **`agents: Mutex<HashMap<String, ActiveAgent>>`**: The core map of all active agent sessions. Protected by a `tokio::sync::Mutex` for safe async access.
- **`agent_order: Mutex<Vec<String>>`**: Maintains the visual order of agents in the UI roster and grid.
- **`input_senders: RwLock<HashMap<String, Sender<Vec<u8>>>>`**: A specialized, lightweight map for routing terminal input. Uses `std::sync::RwLock` to allow low-contention reads for direct text and binary PTY input commands.
- **`workflow_triggers: Mutex<HashMap<String, Vec<JoinHandle<()>>>>`**: Tracks active background tasks (like Cron jobs) for each workflow, allowing for surgical termination (Muting).

## 🤖 ActiveAgent (The Session Handle)
Located in `src-tauri/src/state/active_agent.rs`, this struct represents a single live or hibernating agent session.

### Physical Components:
- **`child_process`**: The actual PTY child process.
- **`pty_master`**: The master handle used for resizing and reading/writing to the terminal.
- **`job_object` (Windows only)**: Ensures that if Wardian crashes, all child processes are immediately cleaned up by the OS.

### Logical Components:
- **`output_buffer`**: A thread-safe string buffer that collects PTY output until the UI drains it.
- **`current_status`**: Real-time status indicator (e.g., "Off", "Idle", "Processing...", "Action Needed"). Live status changes should go through the backend status setter so duplicate observations do not emit duplicate UI events, watch events, or `last_status_at` updates.
- **`query_count`**: Tracks how many prompts have been sent to the agent in the current session.

## 📡 Data Flow
1. **Push**: Agent telemetry (CPU, Memory) is gathered in a background thread and pushed to the UI via the `agent-metrics` event every 5 seconds.
2. **Output Ready Event**: When the PTY reader appends terminal bytes to `output_buffer`, it emits `agent-pty-output-ready` for that session.
3. **Drain**: The UI responds by calling `read_agent_pty` until the command returns `null`, which drains the buffer in-order without a timer-based polling loop.
4. **Live Terminal Host**: The frontend keeps a long-lived xterm instance per session and only detaches or reattaches its DOM host when panes remount, preserving parser and buffer state in memory.
5. **Events**: JSON logs emitted by agents (e.g., via the Gemini CLI's `--output-format stream-json`) are intercepted in the PTY reader thread and emitted as `agent-json-event` for the UI to process.
6. **Startup Replay Boundary**: During app startup, provider log parsing may recover metadata such as query count, log path, resume session, and timestamps, but initial log replay must not create fresh status transitions. Queue completions and CLI `watch --until status:*` evidence come from live transitions after hydration.

## Related Research

- [Local-First State References](../research/local-first-state-references.md)
