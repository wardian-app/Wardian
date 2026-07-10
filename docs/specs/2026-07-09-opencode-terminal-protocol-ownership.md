# OpenCode terminal protocol ownership

- **Status:** Accepted
- **Date:** 2026-07-09
- **Area:** Desktop and remote terminal rendering, provider input, native E2E
- **Issue:** [#665](https://github.com/wardian-app/Wardian/issues/665)
- **Supersedes:** The OpenCode mouse-tracking suppression decisions in
  `2026-06-24-terminal-device-attributes-reply-filter.md` and
  `2026-07-03-opencode-terminal-selection-consistency.md`

## Context

OpenCode 1.17.18 uses OpenTUI as an alternate-screen application. At startup it
enables xterm mouse tracking (`?1000`, `?1002`, `?1003`, and `?1006`) and uses
mouse reports for its conversation scrollbox, selection, hover, and click
handling. OpenTUI also emits synchronized-output frames (`?2026`) so terminal
updates are rendered atomically.

Wardian currently overrides that contract in several conflicting layers:

- the OpenCode input filter treats every mouse button code with motion bit 32 as
  passive, which drops left-button drag code 32 along with no-button motion code
  35 and prevents OpenTUI selection from extending;
- output normalization strips OpenCode's mouse-mode toggles so xterm never
  enters the mouse protocol OpenTUI requested;
- xterm then sees an alternate buffer with no scrollback and converts wheel
  gestures to cursor-up or cursor-down input, which OpenCode maps to prompt
  history;
- desktop wheel tests fabricate normal-buffer scrollback for OpenCode even
  though alternate buffers never have scrollback;
- remote touch handling scrolls only xterm's viewport and cannot drive an
  alternate-screen provider scrollbox; and
- Wardian strips synchronized-output controls and reports mode 2026 as
  unsupported even though the installed xterm 6 renderer supports it.

This sequence explains why individual selection, wheel, and sizing fixes moved
the symptom without restoring the behavior users get when running OpenCode in a
regular terminal.

## Decision

Wardian will preserve OpenCode's native terminal protocol wherever xterm and the
remote attach transport can carry it. OpenCode owns its alternate screen,
internal scroll position, mouse interactions, text selection, clipboard
command, and synchronized frame boundaries. Wardian owns transport, terminal
geometry, lifecycle, and narrowly scoped compatibility filtering.

### Mouse and selection

OpenCode mouse-mode DECSET and DECRST sequences must reach both the desktop
renderer and the remote renderer. Complete SGR mouse reports must be forwarded
unchanged so OpenTUI receives click, drag, hover, and wheel events. OpenCode's
own selection is the supported selection surface; `Ctrl+C` copies that selection
through OpenCode's clipboard behavior, matching a regular OpenCode terminal.

The Windows compatibility guard remains only for malformed legacy passive
motion that has previously arrived as printable coordinate bytes. It may drop a
legacy no-button motion packet (button code 35), but it must preserve drag codes
32 through 34, wheel packets, complete SGR reports, keyboard input, and every
non-OpenCode provider input.

### Wheel and touch ownership

Desktop wheel events in an OpenCode alternate screen are handled by xterm's
negotiated mouse protocol and forwarded to OpenTUI. Wardian must not reinterpret
that gesture as xterm scrollback or cursor-key input.

Remote wheel events use the same xterm protocol once the attachment owns stdin.
For touch-only devices, the remote scroll bridge converts vertical touch travel
into wheel gestures at the xterm event surface when the active buffer is an
alternate screen with mouse tracking. Normal-buffer terminals continue using
xterm viewport scrolling.

### Rendering and geometry

Synchronized-output controls remain in the output stream, and Wardian reports
mode 2026 as supported and reset when OpenCode probes it. xterm therefore
buffers row refreshes until OpenTUI closes each frame instead of rendering
intermediate frame fragments. This should reduce tearing and redundant render
queue work; the existing xterm deadline warning remains diagnostic and is not
silenced.

Geometry decisions use terminal state rather than provider names. Alternate
buffers use xterm's measured cell geometry. Normal buffers may use Wardian's
rendered-row correction when needed to fill the host without blank bottom rows.
Forced remount synchronization continues to ignore stale rendered-row DOM.

### Lifecycle and capability replay

Preserved renderers retain the modes parsed from provider output. A rebuilt
renderer is seeded with serialized terminal state before it accepts interaction.
OpenCode focus-in handling remains available so OpenTUI can re-emit terminal
modes after focus or host transitions.

## Alternatives considered

### Wardian-selected text plus synthesized OpenCode wheel packets

Wardian could continue stripping mouse modes, leave xterm selection enabled,
and manually construct SGR wheel reports. This preserves xterm selection but
requires Wardian to duplicate coordinate encoding and leaves OpenCode click,
drag, hover, and clipboard behavior disabled. Rejected because it is another
split-ownership workaround.

### Wardian shadow scrollback for OpenCode frames

Wardian could parse OpenTUI full-screen repaints and maintain a separate history
buffer. Alternate-screen frames contain only the provider's current viewport,
not the provider's internal scrollbox state, so this cannot reproduce OpenCode
history without reverse-engineering its application model. Rejected.

### Keep the current provider exceptions

Wardian could continue adding provider checks to wheel, sizing, and selection
paths. The current regression chain demonstrates that these checks encode
contradictory ownership assumptions. Rejected.

## Verification

Implementation follows red-green regression coverage in this order:

1. Terminal capability tests prove mouse and synchronized-output controls are
   preserved, mode 2026 is reported as supported, complete SGR reports pass
   through, legacy no-button motion is dropped, and legacy drag is preserved.
2. Desktop terminal tests prove OpenCode alternate-buffer wheel input is left to
   xterm's active mouse protocol and cannot use the fabricated normal-buffer
   scrollback path.
3. Remote terminal tests prove OpenCode startup modes survive snapshot replay,
   wheel input is forwarded while the attachment owns stdin, touch motion is
   translated through the mouse protocol, and normal-buffer touch scrolling is
   unchanged.
4. Sizing tests prove rendered-row correction is selected by active buffer type
   rather than provider name.
5. The native real-provider OpenCode test asserts an alternate buffer with
   active mouse tracking, verifies wheel input changes the conversation viewport
   without changing a draft composer value, and verifies drag plus `Ctrl+C`
   produces OpenCode's copied-selection confirmation.
6. Frontend lint, unit tests, build, screenshot gate, backend checks, and the
   opt-in real OpenCode native target complete the PR verification.

The PR screenshot must show the changed OpenCode interaction state, preferably
conversation history after a wheel scroll with the composer unchanged or the
native copied-selection confirmation.

## Non-goals

- Creating a Wardian-specific OpenCode transcript or scrollback model.
- Changing OpenCode keybindings, prompt-history semantics, or clipboard UX.
- Suppressing xterm performance warnings without addressing measured work.
- Changing mouse behavior for Codex, Claude, Antigravity, Gemini, or mock
  providers.
