# State Management in Wardian

Wardian uses a **Centralized, Thread-Safe Registry** model in the Rust backend to manage the complex lifecycles of multiple autonomous agents.

## 🏛️ AppState (The Global Registry)
Located in `src-tauri/src/state/app_state.rs`, the `AppState` is managed as a Tauri State (`tauri::State<AppState>`).

### Key Fields:
- **`agents: Mutex<HashMap<String, ActiveAgent>>`**: The core map of all active agent sessions. Protected by a `tokio::sync::Mutex` for safe async access.
- **`agent_order: Mutex<Vec<String>>`**: Maintains the visual order of agents in the UI roster and grid.
- **`input_senders: RwLock<HashMap<String, Sender<Vec<u8>>>>`**: A specialized, lightweight map for routing terminal input. Uses `std::sync::RwLock` to allow low-contention reads for direct text and binary PTY input commands.
- **`workflow_triggers: Mutex<HashMap<String, Vec<JoinHandle<()>>>>`**: Tracks active background tasks (like Cron jobs) for each workflow, allowing for surgical termination (Muting).
- **`terminal_sessions: Arc<TerminalSessionBroker>`**: Shares the broker handle
  that owns one actor per PTY runtime, including canonical geometry, runtime
  generation, lease epoch, ordered stream sequence, bounded parser/replay state,
  presentations, and feed consumers.
- **`workbench_io_lock: tokio::sync::Mutex<()>`**: Serializes validated,
  compare-and-swap workbench load/save/reset operations against the two durable
  JSON files.

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
2. **Broker Ingest**: The PTY reader forwards bytes into the terminal-session
   actor. The actor updates its canonical parser and emits monotonically
   sequenced output alongside ordered geometry, ownership, and lifecycle events.
3. **Bounded Fan-Out**: One desktop consumer and independent authenticated
   remote consumers pull bounded event batches. Cursor gaps or generation
   changes return a recovery snapshot instead of accumulating per-view queues.
4. **Independent Presentations**: Each surface/card owns an xterm renderer and
   presentation state. Mirrors fit the owner's canonical grid locally; only the
   explicit lease owner may resize the PTY or send terminal input.
5. **Events**: JSON logs emitted by agents (e.g., via the Gemini CLI's `--output-format stream-json`) are intercepted in the PTY reader thread and emitted as `agent-json-event` for the UI to process.
6. **Startup Replay Boundary**: During app startup, provider log parsing may recover metadata such as query count, log path, resume session, and timestamps, but initial log replay must not create fresh status transitions. Inbox completions and CLI `watch --until status:*` evidence come from live transitions after hydration.

## Workbench State

The frontend Zustand workbench store is the single in-process writer for the
current `WorkbenchDocumentV1`. It does not use Zustand persistence middleware.
The pure command model validates the complete document before and after every
mutation; the navigation service adds registry resolution and transactional
close guards.

Rust owns durable persistence at
`<wardian-home>/settings/workbench.json` with a validated last-known-good
backup. The frontend proposes the next revision and sends the last acknowledged
revision plus an opaque token. Rust alone serializes and hashes the exact bytes.
Conflicts freeze saving without overwriting the local draft.

The document contains split/group/tab placement, bounded surface presentation
state, recently closed entries, and shell dimensions. It excludes agents,
workflows, Inbox/library data, PTY contents, terminal leases, runtime geometry,
DOM focus, drag state, group zoom, credentials, and other recomputable or live
truth. See [Workbench Surfaces](./workbench-surfaces.md) for migration and
versioning rules.
