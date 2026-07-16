# Agents Terminal Stability

## Problem

The tabbed workbench made an Agents surface logically visible even when most of
its cards were outside the scroll viewport. Every terminal card therefore
competed for the process-wide xterm budget. With more than 24 agents, eviction
immediately triggered restoration on another still-logically-visible card,
creating an endless renderer churn loop. The visible symptoms were slow initial
backscroll, repeated terminal repainting, and flicker across the Agents grid.

The terminal broker also reapplied canonical PTY geometry to each presentation's
local xterm and wrote restored snapshots without the provider normalization used
for live output. That violated two existing terminal guarantees: mirrors fit
their own viewport without resizing the PTY, and Codex composer chrome is
recolored consistently on both restored and streamed output.

Finally, watchlist double-click was changed from revealing the agent in the
current Agents surface to opening a separate Agent Session tab.

## Decisions

1. The Agents surface keeps every card in the layout, but only cards intersecting
   (or immediately approaching) the scroll viewport may mount an xterm renderer.
   Off-viewport cards remain suspended broker presentations.
2. Renderer budget eviction is a safety net, not a virtualization mechanism. A
   card that is outside the viewport must not continuously attempt restoration.
3. Canonical broker geometry sizes the parser and native PTY. A presentation's
   browser xterm always fits its own host and is not resized by broker geometry
   events.
4. Broker snapshots use the same provider-aware normalization path as streamed
   output before reaching the browser renderer. Codex composer fills and color
   probes therefore retain the behavior shipped in PRs #530, #544, and #561.
5. Watchlist double-click and Enter reveal and select the agent in the current
   Agents surface. Explicit context-menu actions remain available for opening an
   Agent Session tab or opening one to the side.
6. Dockview reconciliation is driven by canonical document changes, not live
   renderer callback identity. Surface content still receives current telemetry,
   and a group removed by Dockview with its final panel is immediately recreated
   when Wardian's canonical document still owns that empty pane.

## Verification

- Unit tests prove viewport-scoped terminal mounting, budget stability above 24
  agents, local xterm geometry after broker snapshots/events, and Codex snapshot
  recoloring.
- App/watchlist tests prove double-click and Enter reveal the current Agents
  surface without creating an Agent Session tab.
- A 30-agent lifecycle test asserts the mounted set never exceeds the renderer
  budget and hands the released slot to the next near-viewport card.
- Browser E2E covers Agents layout, navigation reveal behavior, and canonical
  empty-pane recovery. PTY lifecycle assertions remain below the browser/native
  boundary in focused terminal tests.
- Native E2E retains responsibility for real PTY resize and first-paint claims.
