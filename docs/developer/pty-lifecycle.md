# PTY Lifecycle and Process Integrity

Wardian is built to handle multiple simultaneous, long-running agent sessions with strict resource and process isolation.

## 🌉 Cross-Platform PTY Layer
Wardian utilizes the `portable-pty` crate to provide a consistent PTY interface across different operating systems.

- **Windows**: Uses **ConPTY** (Windows Pseudo Console) through the `NativePtySystem`.
- **Linux/macOS**: Uses the standard Unix PTY system.

### The PTY Model:
- **Master**: The control end of the PTY, used for reading output and writing input.
- **Slave**: The application end, where the `gemini-cli` (or another agent provider) is spawned.

## 🛡️ Process Integrity (Windows Job Objects)
To prevent "zombie" processes when Wardian crashes or is force-closed, the Windows implementation uses **Job Objects** via the `win32job` crate.

1. On startup, Wardian creates a `win32job::Job`.
2. The `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` flag is enabled.
3. Every agent spawned is "assigned" to this job object.
4. When the Wardian process terminates, the job object is closed by the OS, which automatically kills all processes assigned to it.

## 🔁 Spawning Lifecycle
Spawning an agent follows a deterministic sequence in `manager::spawn_gemini_cli`:

1. **Open PTY**: Create a new master/slave pair.
2. **Build Command**: Configure the command (e.g., `node gemini-cli dist/index.js`) with flags based on the `AgentConfig` (model, sandbox, policy, include directories).
3. **Spawn**: The slave spawns the command.
4. **Piping**:
    - A **Writer Thread** is spawned to handle input from the UI.
    - A **Reader Thread** is spawned to capture output, parsing it for JSON logs and truthiness (status tracking).
5. **Registration**: The `ActiveAgent` handle is added to the `AppState`.

## 📐 Terminal Resizing
Terminal resizing is handled asynchronously in `manager::resize_pty`. When the UI grid layout changes, it invokes a Tauri command that updates the PTY dimensions (`rows` and `cols`) via the `pty_master` handle, ensuring the agent's TUI (like a terminal dashboard) renders correctly.
