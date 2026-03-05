# Wardian Known Issues & Technical Debt

## Bugs
- [ ] **Build Mode Console Flash**: In the production build, initializing an agent spawns an independent `cmd.exe` terminal window during the session ID acquisition process (`obtain_session_id_headless`). This should be suppressed to keep the experience purely within the GUI.

## Technical Debt
- [ ] **State Restoration Synchronization**: Ensure PTY history is correctly re-attached and emitted to the frontend during mass restoration on startup.
- [ ] **Memory Management**: The `output_history` buffer in `ActiveAgent` needs a more sophisticated pruning strategy for extremely long-running sessions.

## UX & UI Improvements
- [ ] **Status Light Calibration**: Finalize colors—Processing is **Yellow**, Action Required is **Orange**.
- [ ] **Advanced List Interactions**: Implement Shift-Click (range selection) and Right-Click context menus for the agent roster.
- [ ] **Theme Support**: Add a "Light Mode" toggle to the application settings.
