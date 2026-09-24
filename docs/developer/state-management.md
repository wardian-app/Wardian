# State Management in Wardian

Wardian uses a **Centralized, Thread-Safe Registry** model in the Rust backend to manage the complex lifecycles of multiple autonomous agents.

## 🏛️ AppState (The Global Registry)
Located in `src-tauri/src/state/app_state.rs`, the `AppState` is managed as a Tauri State (`tauri::State<AppState>`).

### Key Fields:
- **`agents: Mutex<HashMap<String, ActiveAgent>>`**: The core map of all active agent sessions. Protected by a `tokio::sync::Mutex` for safe async access.
- **`agent_order: Mutex<Vec<String>>`**: Maintains the visual order of agents in the UI roster and grid.
- **`input_senders: RwLock<HashMap<String, Sender<Vec<u8>>>>`**: A specialized, lightweight map for routing terminal input. Uses `std::sync::RwLock` to allow low-contention reads for direct text and binary PTY input commands.
- **`automation_triggers: Mutex<HashMap<String, Vec<JoinHandle<()>>>>`**: Tracks active background tasks (like Cron jobs) for each automation, allowing for surgical termination (Muting).
- **`terminal_sessions: Arc<TerminalSessionBroker>`**: Shares the broker handle
  that owns one actor per PTY runtime, including canonical geometry, runtime
  generation, lease epoch, ordered stream sequence, bounded parser/replay state,
  presentations, and feed consumers.
- **`workbench_io_lock: tokio::sync::Mutex<()>`**: Serializes validated,
  compare-and-swap workbench load/save/reset operations against the two durable
  JSON files.
- **`conversation_archive: ConversationArchiveState`**: Owns per-agent archive
  serialization and durable provider-log acquisition cursors. The adjacent
  **`conversation_capture_policy_lock`** serializes global and per-agent logging
  boundaries before callers enter an archive's per-agent gate. The internal
  design record `docs/specs/2026-09-09-provider-log-forward-acquisition.md`
  defines the bounded cursor, continuity, and privacy contract.

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
6. **Startup Replay Boundary**: During app startup, provider log parsing may recover metadata such as query count, log path, resume session, and timestamps, but initial log replay must not create fresh status transitions. Inbox completions come only from live explicit provider turn-completed events with a canonical final assistant response; CLI `watch --until status:*` evidence comes from live transitions after hydration.

### Conversation archive recovery

Conversation archive publication is layered across raw provider observations,
`events.jsonl`, `sources.jsonl`, `conversation.jsonl`, and derived projections.
Recovery first replays durable raw observations in their stored order, restores
their exact artifact references and excerpts, and then admits new observations.
An exact provider observation can be retried idempotently; an ordinary
generated archive call has no stable request ID in the current API, so each
call is a new invocation after any recoverable older observation is restored.
The archive never binds an orphan to a later call by sequence number or matching
text.

If legacy data has ambiguous ownership, a missing generated identity or body,
or a truncated reconstruction payload, recovery fails closed for that archive
with a recoverable error. The failure must not consume unrelated future input.
Recovery repairs durable archive observations and their artifacts; it does not
repair historical user data or invent missing source identity.

## Startup restoration and configuration ownership

Startup restoration uses the same per-agent lifecycle gate as configuration
updates, pause, and resume. It claims the gate before selecting a saved config
for an unregistered agent or publishing its `Restoring` placeholder, and keeps
the claim through the final runtime or error publication. A current registered
agent takes precedence over an older startup snapshot.

An update submitted during restoration waits for that agent's publication, then
validates and persists its changes against the final agent. Success therefore
means the configuration remains current in memory and in
`<wardian-home>/settings/state.json`; a subsequent resume reads those settings.
Paused agents use the same boundary even though their restoration opens no PTY.
Provider startup never holds the global agents, order, or durable roster locks.

### Provider restore ownership across processes

Persisted `last_status = Headless` and `WARDIAN_SESSION_ID` by themselves do
not prove that a provider still owns a conversation. Startup derives headless
status from an unexpired `background_resume` or `background_fresh` conversation
lease. Lease reads used for recovery distinguish a missing lease file from an
unreadable or malformed file; ownership uncertainty withholds provider startup.

Immediately before an interactive provider process is created, Wardian
acquires a cross-process transition lease from the shared conversation lease
store. This rereads lease state under the lease-file lock, so a renewed or newly
acquired background lease blocks the spawn. The transition lease remains held
until provider readiness or a terminal startup status (`Error` or `Off`), and
renews while startup remains pending.

Wardian also scans current process metadata for the configured provider
invocation associated with the exact Wardian session marker or command-line
session identity. This scan is available on Windows, macOS, and Linux when the
OS exposes process arguments and environment. A positive match is only a
**possible provider candidate**: a tool process can inherit the marker and
launch the same provider CLI. It is not proof that Wardian launched or owns the
PID. Candidates withhold restore and are never automatically terminated;
lease expiry is never permission to kill a live process. An unrelated marked
descendant such as `python -m http.server`, or a shell/Node process that merely
mentions the provider name in an argument, does not qualify as a candidate.

If startup reports an existing provider candidate, inspect the reported PID,
executable, command line, and parent process. Stop it through its owning Wardian
instance or normal OS process controls only after confirming its role, then
restart Wardian to retry restoration. Process metadata visibility varies by
OS and permissions, so this scan is a conservative safeguard rather than a
complete cross-OS process identity mechanism. The shared lease store is the
cross-process exclusion mechanism for cooperating Wardian instances.

The deterministic regression in `src-tauri/src/startup_restore/tests.rs` holds
startup completion at a barrier while polling the real configuration command.
It covers paused and live publications without a provider process or timing
delay. Real provider acceptance remains a separate native check.

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
automations, Inbox/library data, PTY contents, terminal leases, runtime geometry,
DOM focus, drag state, group zoom, credentials, and other recomputable or live
truth. See [Workbench Surfaces](./workbench-surfaces.md) for migration and
versioning rules.
