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
- **Cause 2: one concrete defect found and fixed via live-provider debugging.** The
  live Claude audit exposed that user wheel-scrolls were applied and then immediately
  reverted while the provider streamed: drain batches run nearly back-to-back during
  streaming, and `writeTerminalOutputBatch` consulted an at-bottom flag sampled *before*
  its awaited writes, so the post-write `scrollToBottom()` snapped the viewport back —
  the terminal felt unscrollable until the provider went quiet. Diagnosed with
  debug-build wheel branch counters and `onScroll` stack traces surfaced through the
  terminal debug snapshot; fixed by re-checking the live viewport against the pre-write
  base before snapping.
- **The remaining normalization layers** (home-redraw scrollback reconstruction,
  fullscreen-clear-by-newlines rewriting, synthetic scrollback journal — now codex-only)
  are heuristic and version-sensitive. They have test coverage against captured frames,
  and the live rendering audit is the re-validation gate whenever a provider ships a
  renderer change.

## Proposed Decision

1. **Release WebGL contexts deterministically.** `disposeWebglAddonAndReleaseContext`
   captures the addon's canvas before disposal, disposes the addon, then calls
   `canvas.getContext("webgl2").getExtension("WEBGL_lose_context").loseContext()`
   (best-effort, GC remains the fallback). Used by both renderer disposal and LRU
   demotion paths.

   **Follow-up (2026-06-12): the Graph view leaked contexts the same way.** Live
   use still tripped the cap after the terminal fix. Sigma v3 creates *three*
   WebGL contexts per instance (`edges`, `nodes`, `hoverNodes`) and its `kill()`
   only detaches the canvases — `WEBGL_lose_context` appears nowhere in the
   package. Every Graph view visit therefore parked three zombie contexts until
   GC; stacked on the 12-context terminal pool this exceeded Chromium's cap, and
   the forced loss landed on *visible* terminals, dropping them to the DOM
   renderer (where Claude's half-block logo glyphs garble — `customGlyphs` is a
   canvas/WebGL feature). `GraphCanvas` now captures `renderer.getCanvases()`
   before `kill()` and explicitly loses each WebGL context. Context budget:
   12 (terminal pool) + 3 (Graph view while mounted) = 15 of ~16.
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
   false for every provider (the machinery stays behind the switch for one release;
   a live audit pass is the gate for re-enabling it), and all provider output is
   written to xterm natively.

3. **Journal Codex sliding-window drops via direct scrollback insertion.** Live
   capture of Codex 0.139.0 (gpt-5.3-codex-spark) shows it repaints a home-anchored
   window every tick and simply stops painting the top row as content grows — the
   dropped row never scrolls out, so written natively it would vanish. The codex
   normalization journals each repaint frame's dropped rows
   (`reconstructHomeRedrawScrollback`, per frame — extracting across frame boundaries
   glues the status row onto content). `writeTerminalOutputBatch` extracts every
   journal segment mid-stream and inserts the rows directly into xterm's buffer above
   the viewport (`insertSyntheticScrollbackRows`); the legacy `ESC[999;1H` raw
   delivery clamps to the bottom row and corrupts the frame. Three live-disproven
   pitfalls are encoded as regression tests:
   - the journal dedup must consult only scrollback, never the viewport — a genuine
     drop is still visible pre-repaint, and viewport dedup made row survival depend
     on PTY chunk boundaries;
   - the scratch-terminal clone must copy the *rendered* line count — a journal row
     longer than the terminal width wraps, and cloning `rows.length` lines truncated
     the batch tail (one response row vanished whenever a drain batch also journaled
     the long wrapped splash line);
   - per-frame journaling must not be gated on a whole-chunk `extractHomeRedrawLines`
     probe — a chunk opening with a spinner diff frame fails the probe and silently
     skipped every drop the chunk's repaint frames carried.
   Offline replay of captured raw PTY logs (`target/raw-pty-logs`, written by the
   audit harness on failure) reproduced each of these deterministically.

3b. **Scope WebGL contexts to visible terminals; freeze a snapshot on
   demotion (2026-06-12).** Even with deterministic context release, the
   12-context pool bound *total* agent count to the browser cap. Each
   `AgentTerminal` now drives promotion/demotion from an IntersectionObserver:
   a card scrolled out of view (or in a hidden view) releases its context
   after a 1s grace window, and a card entering the viewport promotes
   (focus and maximize promotion unchanged). Before a demoted terminal's
   context is released, its last WebGL frame is copied into a 2D-canvas
   overlay (`preserveDrawingBuffer` is enabled on the addon to make the frame
   readable; 2D canvases do not count against the WebGL cap). The overlay is
   strictly cosmetic — `pointer-events: none`, removed on promotion and the
   moment fresh output arrives — so demoted-but-streaming terminals show live
   DOM rendering rather than a stale still. Result: agent count is unbounded;
   the cap binds only on simultaneously visible terminals, and the DOM
   renderer's font-fallback rendering of custom glyphs (the garbled Claude
   logo) is only ever visible for terminals that are actively streaming while
   demoted. The demotion/promotion lifecycle is provably non-functional-state:
   PTY, parser buffer, input, and the remote gateway never touch the renderer
   (`terminal-visibility-snapshot-native.test.mjs` exercises offscreen
   output/input, re-entry completeness, overlay pointer-transparency, and
   stale-snapshot lift).

3c. **Record PTY size reports that arrive before the PTY exists
   (2026-06-12).** Startup restore publishes "Restoring" placeholders before
   providers spawn (bounded-concurrency restore widened this window). A
   terminal size report for a placeholder used to be dropped — `resize_pty`
   only recorded `pty_sizes` after a successful master resize — so the later
   spawn opened the PTY at the 80×24 default and nothing re-reported, leaving
   full-screen TUIs (OpenCode, Gemini) laid out for 80×24 inside a larger
   grid. `resize_pty` now records the requested size for agents without a
   live PTY and returns Ok; `spawn_agent` already seeds new PTYs from
   `pty_sizes`.

4. **Respect user scroll position during streaming writes.**
   `scrollRendererToBottomAfterWrite` only re-pins the viewport to the bottom when it
   still sits at or past the pre-write base; a wheel scroll landing mid-batch wins.
5. **Accept resize-repaint scrollback duplicates for natively-written providers.**
   Claude/Gemini resize repaints scroll part of the pre-repaint viewport into
   scrollback — the same artifact a standalone terminal shows. A post-write dedup
   (`trimOverlappingScrollbackBeforeViewport` on repaint batches) was tried and
   rejected: after a column reflow the exact-match path cannot fire and the fuzzy
   fallback deletes legitimate history (the audit's completeness gate caught it).
   The rendering audit records such duplicates as warnings for claude/gemini/codex
   (codex resize repaints can re-show journaled rows); completeness of content
   remains a hard failure.

## Verification

Live native-E2E rendering audit (`WARDIAN_E2E_REAL_RENDERING=1`, real provider CLIs):

- **claude** — Claude Code 2.1.173, model `haiku`: all 14 audit states pass
  (initial, settled, narrow, resized, wide, card-maximized/restored,
  minimized/restored, maximized/restored, rapid-resize, scrolled-top,
  cleared-immediate, paused, resumed) including user wheel-scroll and scrollback
  evidence per state. Evidence: `e2e/screenshots/real-provider-rendering/`.
- **codex** — Codex 0.139.0, model `gpt-5.3-codex-spark`: all 14 audit states pass,
  including the 50-row completeness gate on both the initial turn (88×20) and the
  post-clear turn (124×28) — the two geometries where sliding-window drops were
  lost before the journaling fixes in decision 5. Evidence:
  `e2e/screenshots/real-provider-rendering/2026-06-12T00-17-18-037Z`.
- Synthetic scrollback insertion is covered by real-buffer unit tests
  (`syntheticScrollback.test.ts`, bypassing the global headless-xterm mock) and the
  diff-frame-gated journaling by `terminalCapabilities.test.ts`.
- A mock-provider wheel-scroll regression test
  (`e2e-native/tests/terminal-wheel-scroll-native.test.mjs`) reproduces the
  audit layout with back-to-back streaming and narrow/restore window phases.
- The audit harness's maximize helper was fixed to target card controls by
  aria-label; the card header gained a Chat/Terminal toggle as its first button,
  and positional clicking flipped cards into chat mode mid-audit.

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
