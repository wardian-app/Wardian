# Silent Windows Process Launches

## Context

Wardian starts many local processes: provider PTYs, headless provider runs, workflow shell steps, git commands, filesystem junction helpers, updater handoff scripts, explorer reveals, and external editor launches. On Windows, console-backed children can briefly show a separate terminal window when they are started without an explicit hidden/no-window policy. That disrupts Wardian's desktop visuals and has led to call-site-specific fixes.

## Goal

All Wardian-owned terminal and background process launches should be silent on Windows by default. Intentional GUI handoffs must keep their visible behavior.

## Process Categories

1. **PTY-backed terminal processes**: managed agents and the standalone user terminal. These use `portable-pty` and must stay attached to ConPTY while preventing an external console window.
2. **Captured background commands**: provider bootstrap/headless runs, workflow shell and script steps, git, Tailscale status checks, patch scripts, `taskkill`, and junction creation. These should use shared no-window process helpers.
3. **Intentional GUI handoffs**: `explorer`, `open`, `xdg-open`, external editors, and the Windows update handoff. These should remain explicit and should not be accidentally reclassified as background-only launches. On Windows, a GUI handoff may still use the shared silent fire-and-forget process policy for its short-lived command wrapper, such as `cmd /C start` or `code.cmd`, so the target application opens without flashing a console window.

## Design

Wardian already patches `portable-pty` through `vendor/portable-pty`. Keep that Windows ConPTY patch focused on `STARTF_USESHOWWINDOW` and `SW_HIDE`; direct validation showed that adding `CREATE_NO_WINDOW` to ConPTY child creation prevents the standalone user terminal from operating correctly. This is the PTY-specific boundary: direct ConPTY children remain hidden through the startup window hint, while non-PTY background process launches use `CREATE_NO_WINDOW`.

For non-PTY processes, keep `src-tauri/src/utils/process.rs` as the central launcher policy. Captured commands should use `new_silent_command` or `new_silent_std_command` so `stdout` and `stderr` capture semantics stay intact while Windows uses `CREATE_NO_WINDOW`. Fire-and-forget commands should use `new_headless_command` or `new_headless_std_command`, which build on the same silent policy and explicitly null standard handles. Call sites that already need path-candidate fidelity can apply the shared silent-command policy functions to an existing `Command`. Existing GUI handoff call sites remain direct or use their existing handoff-specific flags so the refactor does not suppress user-visible file manager or editor behavior. External editor launches are the important exception: the opened editor or system-default application remains visible, but the Windows command wrapper uses the silent fire-and-forget policy.

The Wardian desktop binary should also use the Windows GUI subsystem in development and release builds. Debug diagnostics should flow through Wardian logging and test output rather than an extra console window attached to the desktop process.

Scripts launched by the Rust backend are part of the same process policy. If a production-reachable Node script starts its own package-manager or shell probes, those child-process calls should set `windowsHide` on Windows so the parent Rust no-window policy is not bypassed by a second-order `cmd.exe` launch.

Provider-owned grandchildren are a separate boundary. Gemini's MCP stdio transport and Claude's MCP config can start configured tools such as `npx chrome-devtools-mcp@latest`; on Windows that can resolve through npm `.cmd` shims even when the transport uses `shell: false`. Wardian cannot wrap those grandchildren directly, so Windows Node-based provider launches add a Wardian-owned preload through `NODE_OPTIONS`. The preload defines the narrow `process.type` marker that Gemini's bundled MCP transport already checks before enabling `windowsHide`, and also patches Node's `child_process` launch helpers to default provider-owned grandchildren to `windowsHide: true`. The shared Windows provider preload is written under `WARDIAN_HOME/runtime/windows/` and is applied only to Wardian-launched providers that need it.

Antigravity uses the same user-level Gemini MCP configuration but runs as a native provider process, so `NODE_OPTIONS` does not reach the first MCP child launch. When Wardian launches Antigravity on Windows, it normalizes npm `npx` MCP entries in `<user-home>/.gemini/config/mcp_config.json` and known Antigravity MCP config candidates to `node <wardian-home>/runtime/windows/wardian-npx.cjs ...`. The wrapper loads the shared preload and then delegates to npm's `npx-cli.js`, preserving the configured MCP arguments while bypassing the `.cmd` shim that would otherwise create a visible console window. Existing direct `node` MCP entries are left unchanged.

## Non-Goals

- Do not change provider arguments, current working directories, environment variables, stdout/stderr capture, or process lifecycle semantics.
- Do not hide or redirect intentional GUI actions.
- Do not introduce a new PTY abstraction unless `portable-pty` exposes a stable creation-flag API later.

## Verification

- Unit tests for the centralized Windows process flag helpers, launch specs, and captured stdout/stderr behavior.
- Existing Rust tests around provider launch argument construction and shell selection.
- Rust coverage for the shared Windows provider preload, `NODE_OPTIONS` preservation, and Antigravity MCP `npx` normalization.
- Native user-terminal smoke to prove the ConPTY path still runs commands after the process-creation policy change.
- `cargo check`, `cargo test`, and `cargo clippy` in `src-tauri`.
- Frontend validation remains unchanged because this is backend/runtime infrastructure only.
