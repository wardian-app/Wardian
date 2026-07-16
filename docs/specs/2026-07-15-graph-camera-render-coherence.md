# Graph camera render coherence

## Problem

Wheel zoom could visibly jitter even though Sigma's camera animation was smooth.
Wardian had two render paths that could disagree with Sigma during the same
animation frame:

1. `GraphCanvas` cleared and repopulated the Graphology instance already bound
   to Sigma. Each mutation emitted renderer work, exposing incomplete graph
   states while the camera was moving.
2. `EdgeActivityOverlay` rendered on its own animation-frame loop and used
   Sigma's cached projection matrix. A camera update can run the overlay frame
   before Sigma refreshes that cache, leaving activity edges one frame behind
   their nodes.

This is a renderer-ownership problem, not a transition or CSS problem. Adding
debounces or suppressing camera events would hide frames while leaving the two
render paths incoherent.

## Decision

Graph projection changes are assembled in a new, detached Graphology graph.
After the complete node and edge projection exists, `GraphCanvas` installs it
with one `Sigma.setGraph` call. Wardian never clears or incrementally rebuilds
the renderer-bound graph.

For each activity-overlay frame, Wardian reads the current camera state and
builds one projection matrix with Sigma's current viewport, graph dimensions,
and stage padding. Every overlay endpoint in that frame uses that same matrix.
This removes callback-order dependence without recomputing a matrix per edge.

Theme-only changes still rebuild the detached graph so resolved CSS-variable
colors stay correct. The Sigma label setting is updated only when its resolved
value actually changes, avoiding an unnecessary scheduled refresh during
ordinary projection updates.

## Invariants

- A renderer-bound Graphology graph is immutable from Wardian's point of view.
- One logical projection update causes one atomic graph installation.
- All Canvas2D overlay geometry in one frame uses one current-camera matrix.
- Telemetry changes excluded from the visual projection do not rebuild Sigma.
- Camera reset, pointer interactions, and the camera instance survive graph
  projection swaps.

## Verification

Unit coverage must prove that projection changes call `setGraph` without
calling `graph.clear`, telemetry-only changes do not reinstall the graph, theme
changes re-resolve colors, and overlay endpoints receive the matrix derived
from the current camera state. Browser smoke coverage must exercise repeated
wheel zoom on the real Graph surface and confirm that the surface remains
mounted at stable dimensions without runtime errors.
