# Workbench Tab Strip Lifecycle

## Decision

Every canonical Wardian pane always exposes its surface tab strip at the top of
the pane. Dockview's pane viewport is structural layout, not a surface scroll
container. Its overflow remains locked while each surface provides the scroll
region appropriate to its content.

Agent reveal and query navigation scroll the Agents overview container
directly. They must not call `scrollIntoView()` on an agent card because that
API may scroll every eligible ancestor, including Dockview's pane viewport.
When the pane viewport scrolls by the tab-strip height, both the tab strip and
the keep-alive overlay move, making the overlay appear to replace the tabs.

## Verification

- Revealing a distant agent leaves the Dockview pane viewport at `scrollTop = 0`.
- The active keep-alive overlay remains aligned with the pane content below the
  36px tab strip.
- Repeated keep-alive tab changes and viewport resizes preserve that alignment.
