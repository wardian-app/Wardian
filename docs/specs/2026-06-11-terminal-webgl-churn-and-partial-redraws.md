# Terminal WebGL Context Churn and Partial Provider Redraws

Filename: `2026-06-11-terminal-webgl-churn-and-partial-redraws.md`

- **Status:** Implemented
- **Date:** 2026-06-11

## Context and Problem Statement

Wardian still struggles when many TUIs render at once. Three suspected root causes were
investigated:

1. **"Too many active WebGL contexts" warnings.** Each agent terminal gets its own
   `@xterm/addon-webgl` context. A 12-entry LRU pool already bounds *live* contexts below
   Chromium's ~16-context cap, yet the warning persisted.
2. **Embedded-PTY rendering glitches** in TUIs such as Codex, despite a layer of
   provider-specific output normalization.
3. **Quirk fixes written against older TUI versions** (Codex, Claude, OpenCode) that may
   now do more harm than good. One user reported large blank screens for Claude: a mostly
   black terminal showing only the token-count status row and cursor.

Findings:

- **Cause 1 confirmed as a zombie-context leak.** `@xterm/addon-webgl` removes its canvas
  on `dispose()` but never calls `WEBGL_lose_context.loseContext()` (verified against the
  vendored addon source — no occurrence in the package). Chromium counts a context as
  active until the detached canvas is garbage-collected, so every pool eviction, grace
  disposal, or re-promotion stacked a zombie context on top of the 12 live ones and
  tripped the cap. The cap then force-loses the *oldest* context, which can belong to a
  terminal the user is looking at. Consolidating every PTY into one shared canvas was
  considered and rejected: xterm.js's renderer owns its canvas per terminal instance, so
  a shared-context architecture would mean replacing the renderer wholesale; releasing
  contexts deterministically keeps the live pool (12) safely under the cap.
- **Cause 3 confirmed for the Claude blank screens.** `providerUsesViewportRedraws`
  routes any claude/gemini/codex chunk containing a top-left cursor address (`ESC[H`)
  or `ESC[2J` through `applyViewportRedrawInPlace(..., preserveExistingViewport: false)`,
  which renders the chunk into a *blank* scratch screen and replaces the whole viewport.
  Codex repaints full frames, so this is safe for it. Claude (and Gemini) emit
  home-anchored frames that may repaint only part of the screen — and a frame can split
  across PTY reads, since drain batching is not frame-aligned. Rendering such a partial
  frame into a blank scratch wipes every row the frame didn't touch, leaving exactly the
  reported symptom: a blank screen with only the status/token row.
- **Cause 2 remains open.** The remaining normalization layers (home-redraw scrollback
  reconstruction, fullscreen-clear-by-newlines rewriting, synthetic scrollback journal)
  are heuristic and version-sensitive. They have test coverage against captured frames
  from Claude Code v2.1.x and current Codex, but should be re-validated against live
  providers via the native E2E harness whenever a provider ships a renderer change.

## Proposed Decision

1. **Release WebGL contexts deterministically.** `disposeWebglAddonAndReleaseContext`
   captures the addon's canvas before disposal, disposes the addon, then calls
   `canvas.getContext("webgl2").getExtension("WEBGL_lose_context").loseContext()`
   (best-effort, GC remains the fallback). Used by both renderer disposal and LRU
   demotion paths.
2. **Restrict the viewport-redraw machinery to Codex; write Claude/Gemini streams
   natively.** An intermediate fix (seeding the scratch screen with the existing
   viewport for claude/gemini) was tried first and disproved by a live native-E2E run
   against Claude Code 2.1.173 (Haiku): the journaled scrollback contained rows like
   `▐▛14█▜▌   Claude Code v2.1.173` — numbered output cell-merged with stale banner
   content. The live capture shows modern Claude Code is a *diff renderer*: it
   cursor-addresses only the changed cells of a row and assumes the terminal retained
   its previous frame. Scratch-screen replacement breaks that contract both ways — a
   blank scratch wipes unwritten cells (the reported blank screens), a preserved
   scratch merges the frame with rows Claude believes it already replaced. xterm itself
   honors the retained-frame contract, so `providerUsesViewportRedraws` now returns
   true only for codex (full-frame repainter), and claude/gemini output is written to
   xterm natively.

## Consequences

- **Positive**: Pool evictions and renderer disposals free their context slot
  immediately; the live count stays at ≤12 with no zombie overhang, eliminating the
  forced loss of visible terminals' contexts.
- **Positive**: Claude/Gemini diff-rendered frames (partial repaints, frames split
  mid-redraw across PTY reads) can no longer blank or merge viewport rows; xterm
  applies them exactly as a standalone terminal would.
- **Negative**: Claude/Gemini lose the synthetic scrollback journal that the redraw
  path provided. If a current Claude/Gemini build still discards completed rows
  without scrolling them out, that history is not reconstructed. Live-provider audit
  evidence is the gate for re-adding any such heuristic.
- **Negative**: The broader quirk-fix layer is still heuristic; provider renderer
  upgrades can invalidate it silently. Re-validation against live providers (native
  E2E + real-provider harness) is the standing mitigation.
