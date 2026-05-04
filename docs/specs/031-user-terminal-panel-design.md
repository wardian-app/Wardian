# Spec 031: User Terminal Panel Design

- **Status:** Implemented
- **Date:** 2026-05-04
- **Decider:** Wardian maintainers

## Goal

Add a standalone user terminal to Wardian. The terminal is for the human user, not an agent PTY. It opens from the sidebar rail, appears at the bottom of the main view, and can jump to a selected agent's workspace when exactly one agent is selected.

## Decisions

- Use a separate user-terminal PTY subsystem instead of reusing agent terminal state.
- Keep `Wardian.exe` as the desktop app entry point; this design does not affect CLI binaries.
- The terminal rail button toggles the bottom panel without changing the active sidebar tab.
- The panel reserves layout space and shrinks the main view above it.
- The shell process stays alive when the panel is hidden.
- Closing Wardian cleans up the shell process.
- Default cwd is Wardian home, honoring `WARDIAN_HOME`; release normally resolves to `~/.wardian`.
- A selected-agent workspace button is enabled only when exactly one agent is selected and its `folder` is available.

## Architecture

### Backend

Add a single user-terminal session to `AppState`. It should be independent from `ActiveAgent`, agent input senders, and agent terminal events.

The user terminal uses `portable-pty`, matching Wardian's PTY integrity requirement, but it does not share provider-specific agent terminal code. It should reuse shell discovery and shell settings from `src-tauri/src/utils/shell.rs`, with a new interactive-shell launch path because the existing shell helpers are command-execution oriented.

Proposed Tauri commands:

- `ensure_user_terminal`
- `send_input_to_user_terminal`
- `send_binary_input_to_user_terminal`
- `resize_user_terminal`
- `read_user_terminal_pty`
- `restart_user_terminal`
- `set_user_terminal_cwd`

The reader task appends PTY output to a drain-on-read buffer and emits `user-terminal-output-ready`.

### Frontend

`SidebarIconRail` treats Terminal as a special toggle, not a `SidebarTab`. The button can show an open state, but it must not call `setActiveTab("terminal")`.

`SidebarContentPane` removes the current terminal placeholder pane.

`App.tsx` owns:

- whether the user terminal panel is open
- terminal panel height
- selected-agent workspace resolution
- rail toggle wiring

Add `features/terminal/UserTerminalPanel.tsx` for the bottom panel and xterm host.

## UI Behavior

The bottom terminal panel:

- Defaults to about `34vh`.
- Has a draggable top edge.
- Persists height and open/closed state through the existing layout/settings persistence pattern.
- Uses existing terminal font settings.
- Provides compact header controls for hide, restart, and cd-to-selected-workspace.

The main view should use flex layout so the terminal panel consumes bottom space and the active view above keeps a stable minimum height. The panel should not overlay or hide agent cards, workflow controls, or library content.

## Data Flow

Launch flow:

1. User clicks the terminal rail button.
2. `App.tsx` opens the bottom panel without changing `activeTab`.
3. `UserTerminalPanel` mounts and calls `ensure_user_terminal` with initial cols and rows.
4. Rust resolves the configured shell for interactive use.
5. Rust launches the shell in Wardian home and stores the user-terminal session.
6. PTY output is buffered and signaled via `user-terminal-output-ready`.
7. The frontend drains output with `read_user_terminal_pty`.

Input flow:

1. xterm input calls `send_input_to_user_terminal`.
2. Binary/control input uses `send_binary_input_to_user_terminal` where needed.
3. resize/fit events call `resize_user_terminal`.

Workspace jump flow:

1. `App.tsx` resolves exactly one selected agent.
2. The frontend passes that agent's `folder` to `set_user_terminal_cwd`.
3. Rust validates the path exists.
4. Rust writes a shell-specific cd command into the live PTY:
   - PowerShell/pwsh: `Set-Location -LiteralPath '...'`
   - cmd: `cd /d "..."`
   - POSIX shells: `cd '...'`

## Error Handling

- If no compatible shell exists, keep the panel open and show an error with retry.
- If the shell exits, keep the panel open with an exited state and restart action.
- If the selected workspace path is invalid, show a concise inline error in the terminal header/status area.
- If the input channel is full, surface a non-blocking error and leave the shell running.
- Dropping the user-terminal session kills the PTY child and cleans up platform-specific process resources.

## Testing

Frontend unit tests:

- Terminal rail button opens the bottom panel.
- Clicking Terminal preserves the previous active sidebar pane.
- Terminal button reflects open state independently from sidebar `activeTab`.
- Workspace button is enabled only for a single selected agent with a workspace.
- Restart and hide controls call the expected handlers.

Rust unit tests:

- Interactive shell launch resolution does not use one-shot command flags such as `-Command`, `-c`, or `/C`.
- Shell-specific cd commands quote paths correctly for PowerShell, cmd, and POSIX shells.
- Invalid workspace paths are rejected before writing into the PTY.

Native E2E tests:

- Open the app, open the terminal panel, run a simple command, and verify output.
- Select one agent, click cd-to-workspace, run a cwd command, and verify it matches the agent workspace.

Browser E2E may cover panel layout and toggling, but real PTY behavior belongs in native E2E.

## Documentation

After implementation, update agent-facing dev/e2e instructions to state that real terminal behavior requires native E2E. Browser E2E can prove layout and interaction wiring only.
