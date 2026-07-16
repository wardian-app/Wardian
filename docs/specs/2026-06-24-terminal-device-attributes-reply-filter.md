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
passes over an OpenCode terminal card. Earlier mitigation disabled OpenCode
xterm stdin while the card was not selected. That behavior was superseded by the
provider-scoped input guard described below, which removes passive mouse-motion
garbage without making OpenCode interactivity depend on selection.

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

OpenCode terminal cards keep xterm stdin enabled even when the card is not
selected, matching other providers. Wardian-originated injections still call the
Tauri input commands directly, and frontend terminal input is filtered before
PTY forwarding when a provider-specific terminal report or passive mouse-motion
guard applies.

## Update 2026-07-02: OpenCode mouse-motion input guard

The selected or maximized OpenCode terminal still has xterm stdin enabled, so
OpenCode can enable mouse tracking for its in-TUI scroll surface. On Windows,
passive pointer movement over that active terminal can produce legacy xterm
mouse-motion bytes whose coordinate fields are printable ASCII, for example
`CFE`, `GFF`, or adjacent descending punctuation. If those bytes reach
OpenCode's composer instead of being consumed as mouse protocol, they appear as
random typed characters while the user moves the pointer.

Wardian now filters OpenCode passive mouse-motion reports at the frontend input
boundary before forwarding `onData` or `onBinary` to the PTY. The filter is
provider-scoped to OpenCode, keeps normal keyboard input, and keeps non-motion
mouse packets such as wheel reports so OpenCode-owned scrolling remains
available. Remote terminal attach uses the same filter before sending input
frames over the attach websocket.

## Update 2026-07-03: OpenCode selection gate removed

OpenCode no longer has a selection-only stdin exception. Terminal cards now keep
the existing Wardian default behavior: mouse and wheel input can reach both the
main app's viewport handling and the terminal input path without requiring the
terminal card to be selected first. OpenCode passive mouse-motion reports remain
filtered by `filterProviderTerminalInput`, so preserving direct terminal
scrolling does not reintroduce composer garbage characters.

## Update 2026-07-07: OpenCode mouse tracking suppressed for text selection

> **Superseded 2026-07-09:** See
> `2026-07-09-opencode-terminal-protocol-ownership.md`. OpenCode mouse tracking
> is preserved so OpenTUI can own scrolling and native selection; the Windows
> compatibility filter is narrowed to malformed legacy no-button motion.

OpenCode can also enable xterm mouse-tracking modes with DECSET toggles such as
`ESC[?1000h`, `ESC[?1002h`, `ESC[?1003h`, `ESC[?1006h`, and `ESC[?1016h`.
When xterm.js enters those modes, plain drag gestures become mouse protocol
input and normal text selection is unavailable unless the user uses a
terminal-specific modifier.

Wardian now strips those OpenCode-only mouse-tracking toggles from rendered
output before xterm sees them. Terminal stdin remains enabled, keyboard input
and capability replies are unchanged, and the passive mouse-motion input guard
remains as defense in depth. OpenCode-owned mouse tracking is no longer
preserved in Wardian terminals because selectable terminal text is the expected
default interaction.

## Verification

Focused regression coverage lives in
`src/features/terminal/terminalCapabilities.test.ts` and proves:

- xterm's `ESC[?1;2c` primary Device Attributes reply is stripped from Codex
  input;
- brokered providers strip generated terminal report replies while unbrokered
  providers retain them;
- echoed Codex primary Device Attributes replies are stripped from rendered
  output;
- OpenCode terminal stdin remains enabled even when the card is not selected,
  while Wardian's capability broker still injects required terminal replies.
- OpenCode passive mouse-motion reports, including bare binary triplets that
  match the visible composer garbage pattern, are dropped before PTY forwarding
  while non-OpenCode provider input is preserved.
- OpenCode mouse-tracking toggles are stripped from rendered output before
  xterm.js can enter mouse-reporting mode.
