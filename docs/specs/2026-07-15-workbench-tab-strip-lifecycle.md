# Workbench Tab Strip Lifecycle

## Decision

Every canonical Wardian pane always exposes its surface tab strip at the top of
the pane. Dockview may retain group-local header state while panels move,
maximize, resize, or reconcile. That library-local state is not authoritative:
Wardian repairs a retained group's header to visible and top-positioned during
both canonical document projection and Dockview-only layout changes.

The repair runs even while Wardian's projection guard is active. The guard may
suppress layout feedback into the persisted split model, but it must never
suppress restoration of required window chrome.

## Verification

- A retained group whose header is hidden and moved away from the top recovers
  on a Dockview layout event without a Wardian document mutation.
- Canonical reconciliation continues to create every group with a visible top
  header.
- The active surface remains mounted while its tab strip is repaired.
