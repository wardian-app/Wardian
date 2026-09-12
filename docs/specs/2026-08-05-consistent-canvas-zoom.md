# Consistent Canvas Zoom

## Decision

Garden and Graph use the same wheel zoom contract:

- A traditional 120-pixel wheel notch changes zoom by 8%.
- High-resolution wheel and trackpad deltas scale that factor proportionally.
- The world point under the pointer remains fixed while the view zooms.
- Wheel zoom is applied continuously from each input event rather than through
  a coarse, debounced animation step.

Garden applies the viewport transform imperatively through Konva. React owns the
zoom readout and rendering detail, but does not reapply a stale transform to the
Stage during a wheel gesture. Garden and its parent view clip the canvas so a
transformed map cannot create an ancestor scrollbar and feed a transient width
change back into the pane layout.

Graph intercepts the native wheel event before Sigma's default captor. It uses
Sigma's cursor-anchored camera calculation and a one-frame camera animation so
each delta is applied immediately while any already-running reset animation is
interrupted; this avoids Sigma's default 1.5x animated/debounced steps.

## Verification

The shared wheel-factor unit tests cover ordinary, high-resolution, and invalid
deltas. Garden and Graph component tests cover pointer anchoring and the live
canvas transforms. The focused suites and frontend lint/build are run as part of
the change validation.
