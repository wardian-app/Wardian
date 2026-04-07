# PTY Lifecycle and Process Integrity

Wardian is built to handle multiple simultaneous, long-running agent sessions with strict resource and process isolation.

## 🌉 Cross-Platform PTY Layer
Wardian utilizes the `portable-pty` crate to provide a consistent PTY interface across different operating systems.

- **Windows**: Uses **ConPTY** (Windows Pseudo Console) through the `NativePtySystem`.
- **Linux/macOS**: Uses the standard Unix PTY system.

### The PTY Model:
- **Master**: The control end of the PTY, used for reading output and writing input.
- **Slave**: The application end, where the selected runtime shell hosts the provider command.

## 🛡️ Process Integrity (Windows Job Objects)
To prevent "zombie" processes when Wardian crashes or is force-closed, the Windows implementation uses **Job Objects** via the `win32job` crate.

1. On startup, Wardian creates a `win32job::Job`.
2. The `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` flag is enabled.
3. Every agent spawned is "assigned" to this job object.
4. When the Wardian process terminates, the job object is closed by the OS, which automatically kills all processes assigned to it.

## 🔁 Spawning Lifecycle
Spawning an agent follows a deterministic sequence in `manager::spawn_agent`:

1. **Open PTY**: Create a new master/slave pair.
2. **Resolve Runtime Shell**: Select the configured shell profile (`Auto`, discovered shell, or `Custom`).
3. **Build Provider Command**: Assemble the provider executable plus provider-specific flags from the `AgentConfig`.
4. **Wrap for Host Shell**: Convert the provider command into a shell-hosted invocation that respects the selected shell family.
5. **Spawn**: The PTY slave spawns the shell-hosted command.
6. **Piping**:
   - A **Writer Thread** is spawned to handle input from the UI.
   - A **Reader Thread** is spawned to capture output, parsing it for JSON logs and status transitions.
7. **Registration**: The `ActiveAgent` handle is added to the `AppState`.

### Shell-hosted Launch Notes
- Workflow shell-command nodes and headless provider runs use the same shell resolver as interactive PTY sessions.
- On Windows, `.cmd` and `.bat` provider shims may be re-routed through `cmd.exe` when the selected host shell is PowerShell, Git Bash, or WSL.
- On Linux and macOS, Wardian resolves shells from the standard shell list and executes the provider command through that shell's command-string mode.

## Testing Boundaries

PTY behavior cannot be validated by browser-only UI tests.

- Browser Playwright smoke tests are useful for layout, navigation, and non-native UI regressions.
- Native Tauri runtime tests are required for:
  - Tauri `invoke` behavior
  - PTY-backed terminal rendering
  - provider spawn and resume behavior
  - shell-hosted process launch behavior

When debugging or testing PTY issues, treat browser smoke results as insufficient evidence. Use the native runtime harness for any claim about terminal or provider behavior.

## 📐 Terminal Resizing
Terminal resizing is handled asynchronously in `manager::resize_pty`. When the UI grid layout changes, it invokes a Tauri command that updates the PTY dimensions (`rows` and `cols`) via the `pty_master` handle, ensuring the agent's TUI renders correctly.
