# Terminal Device Attributes reply filtering

- **Status:** Implemented
- **Date:** 2026-06-24
- **Area:** Frontend terminal capability broker, Windows modern ConPTY provider sessions

## Problem

After Wardian switched Windows agent sessions to the bundled modern ConPTY, Codex
sessions gained correct native scrollback, but some launches still showed stray
terminal report text such as `[?1;2c`. OpenCode could show similar report text
when terminal cards were scrolled past or reactivated.

`ESC[?1;2c` is the terminal's primary Device Attributes reply. A provider emits
a primary Device Attributes query (`ESC[c`), xterm.js auto-generates the reply on
its `onData` channel, and Wardian was forwarding that generated reply back into
the provider. Under modern ConPTY, the native terminal side can already answer
these probes, and Wardian explicitly brokers terminal capability replies for
OpenCode and Antigravity. The forwarded xterm duplicate can therefore be echoed
back into provider output and appear as visible report text.

A separate OpenCode path is not spawn-bound: OpenCode owns its in-TUI scrolling
surface and xterm can emit mouse/wheel protocol input while the pointer merely
passes over an OpenCode terminal card. When that card is not selected, the user
intent is viewport navigation, not provider interaction, so passive xterm stdin
must not reach the provider.

## Decision

Wardian filters xterm-generated terminal report replies before forwarding
frontend `onData` input to providers whose terminal capabilities are already
handled by Wardian or modern ConPTY:

- Codex: modern ConPTY answers terminal probes natively.
- OpenCode and Antigravity: Wardian's capability broker sends the supported
  terminal replies explicitly.

Providers outside that brokered set keep the existing behavior so Wardian does
not remove xterm-generated replies they may still rely on.

Codex output normalization also strips echoed primary Device Attributes replies,
matching the existing output-side cleanup for echoed color and light-dark report
replies.

OpenCode terminal cards disable xterm stdin while the card is not selected.
xterm documents that `disableStdin` also prevents mouse events from being
emitted by the terminal, which blocks passive wheel/mouse reports while the user
scrolls around the grid. Wardian-originated injections still call the Tauri input
commands directly, so capability-broker replies, control sends, and CLI sends do
not depend on xterm stdin and remain active for unselected cards.

## Verification

Focused regression coverage lives in
`src/features/terminal/terminalCapabilities.test.ts` and proves:

- xterm's `ESC[?1;2c` primary Device Attributes reply is stripped from Codex
  input;
- brokered providers strip generated terminal report replies while unbrokered
  providers retain them;
- echoed Codex primary Device Attributes replies are stripped from rendered
  output;
- unselected OpenCode terminals disable xterm stdin while Wardian's capability
  broker still injects required terminal replies.
