# State Management in Wardian

Wardian uses a **Centralized, Thread-Safe Registry** model in the Rust backend to manage the complex lifecycles of multiple autonomous agents.

## 🏛️ AppState (The Global Registry)
Located in `src-tauri/src/state/app_state.rs`, the `AppState` is managed as a Tauri State (`tauri::State<AppState>`).

### Key Fields:
- **`agents: Mutex<HashMap<String, ActiveAgent>>`**: The core map of all active agent sessions. Protected by a `tokio::sync::Mutex` for safe async access.
- **`agent_order: Mutex<Vec<String>>`**: Maintains the visual order of agents in the UI roster and grid.
- **`input_senders: RwLock<HashMap<String, Sender<String>>>`**: A specialized, lightweight map for routing terminal input. Uses `std::sync::RwLock` to allow zero-contention reads by the terminal input event listener.
- **`workflow_triggers: Mutex<HashMap<String, Vec<JoinHandle<()>>>>`**: Tracks active background tasks (like Cron jobs) for each workflow, allowing for surgical termination (Muting).

## 🤖 ActiveAgent (The Session Handle)
Located in `src-tauri/src/state/active_agent.rs`, this struct represents a single live or hibernating agent session.

### Physical Components:
- **`child_process`**: The actual PTY child process.
- **`pty_master`**: The master handle used for resizing and reading/writing to the terminal.
- **`job_object` (Windows only)**: Ensures that if Wardian crashes, all child processes are immediately cleaned up by the OS.

### Logical Components:
- **`output_buffer`**: A thread-safe string buffer that collects PTY output for the UI to poll.
- **`current_status`**: Real-time status indicator (e.g., "Idle", "Processing", "Action Needed").
- **`query_count`**: Tracks how many prompts have been sent to the agent in the current session.

## 📡 Data Flow
1. **Push**: Agent telemetry (CPU, Memory) is gathered in a background thread and pushed to the UI via the `agent-metrics` event every 5 seconds.
2. **Pull**: The UI polls for terminal output via the `read_agent_pty` command, which "drains" the `output_buffer` to minimize memory usage.
3. **Events**: JSON logs emitted by agents (e.g., via the Gemini CLI's `--output-format stream-json`) are intercepted in the PTY reader thread and emitted as `agent-json-event` for the UI to process.
