# Claude Native Scrollback For Wardian Terminals

## Context

Claude Code 2.1.132 added `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1` as an opt-out from fullscreen alternate-screen rendering so conversation output remains in the terminal's native scrollback. Wardian's mobile PWA terminal scrolls by translating drag gestures into xterm scrollback movement. When Claude owns the alternate screen, the rendered conversation can remain visible while the scrollback surface has no history to drag through.

## Decision

Wardian sets `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1` for Claude provider launches that run inside Wardian-managed terminal surfaces. The same runtime environment helper also keeps `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`, preserving native `CLAUDE.md` discovery from Wardian's configured additional directories.

The setting is applied to embedded PTY launches, process-based Claude launches, and copyable external Claude commands so copied sessions behave like Wardian-managed sessions.

This keeps compatibility with both Claude terminal behaviors Wardian has to handle: older or non-alternate-screen Claude sessions continue to run with the existing `CLAUDE.md` discovery environment, while newer Claude Code sessions that support alternate-screen rendering receive the documented opt-out needed for native scrollback.

## Verification

- Rust unit coverage asserts the embedded Claude runtime environment includes the alternate-screen opt-out.
- The copyable external command smoke test logs and verifies the same environment before invoking a fake Claude executable.
