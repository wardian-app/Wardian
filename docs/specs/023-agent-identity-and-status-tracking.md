# Spec 023: Agent Identity and Status Tracking

## 1. Context & Motivation

Wardian currently manages Agent Identity (`session_name`) and Agent Status (`current_status`) as mutable fields in memory (`AppState`). This creates several issues:

1.  **Duplicate Names:** Agents can have the same `session_name`, making them impossible to uniquely identify via CLI.
2.  **Volatile Status Tracking:** Status is tracked by parsing stdout and updating an in-memory `HashMap`. If Wardian restarts, it cannot recover the true state of background processes.
3.  **Process Lifecycle Coupling:** The UI assumes the agent process is tied to the Tauri application lifecycle.

## 2. Agent Identity: Globally Unique CLI-Friendly Names

The UUID (`session_id`) remains the internal primary key and folder identifier (`~/.wardian/agents/<uuid>`). The `session_name` becomes a **globally unique CLI-friendly alias**.

### 2.1. Naming Constraints
-   **Regex:** `^[a-zA-Z0-9_-]+$` (No whitespace, no special characters).
-   **Uniqueness:** The Rust backend enforces uniqueness upon creation and mutation.
-   **Auto-Generation:** Collisions result in unique suffixes (e.g., `coder-a1b2`).

### 2.2. Folder Stability
-   Folder paths remain tied to UUIDs. Renaming an agent is a metadata-only change, ensuring no broken paths or file locks.

## 3. Status Tracking: SQLite Event Database

A local **SQLite Database** (`~/.wardian/state.db`) acts as the source of truth, enabling process recovery and event auditing.

### 3.1. Schema & Performance
-   **WAL Mode:** SQLite will use Write-Ahead Logging (WAL) to prevent blocking concurrent UI reads during high-frequency parser writes.
-   **`agents` table:** Added `last_status` and `last_pid` columns for O(1) status retrieval.
-   **`events` table:** Audit log of status changes and tool calls.
-   **Pruning:** A background task will prune the `events` table (e.g., keep last 1000 events or 30 days of history).

### 3.2. Status Lifecycle & Deduplication
-   **Deduplication:** Status updates are only written to SQLite if `new_status != last_known_status` to prevent write amplification.
-   **Event Source:** PTY parsers emit `status_change` events.

### 3.3. Process Recovery & "Headless" Reality
-   **Forensic ID Injection:** Every agent is spawned with `WARDIAN_SESSION_ID=<uuid>` in its environment.
-   **Recovery:** On startup, Wardian reconciles the DB with the OS process list. It verifies process identity by checking the environment block (via `sysinfo`).
-   **Headless Capability:** If a process is alive but Wardian lacks the Master PTY handle (destroyed on restart), the status is `Headless`.
    -   *Constraint:* "Headless" agents are **unreachable for interactive input** until a PTY daemon (e.g., tmux-style persistence) is implemented. They continue executing tasks but cannot be commanded via the terminal grid.

## 4. Implementation Phasing

**Phase 1: Strict Identity**
1.  Update UI to enforce CLI-friendly name regex.
2.  Update Rust backend to reject duplicate names.
3.  Migration to sanitize existing names in `projects.json`.

**Phase 2: SQLite Infrastructure**
1.  Add `rusqlite` with WAL mode.
2.  Implement `db` module for migrations and `WARDIAN_SESSION_ID` injection.
3.  Hook parsers into the DB with a deduplication layer.

**Phase 3: State Decoupling**
1.  Derive telemetry and status from DB + `sysinfo`.
2.  Implement event pruning logic.
