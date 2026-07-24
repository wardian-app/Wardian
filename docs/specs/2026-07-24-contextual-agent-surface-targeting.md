# Contextual Agent Surface Targeting

**Date:** 2026-07-24
**Status:** Implemented

## Decision

Agent-open actions from Graph, Garden, and Inbox reuse a directly adjoining,
visible Agent Session when one exists. The target presentation is rebound to
the selected agent and focused, rather than adding another session tab.

This makes a two-pane investigation layout behave as one working context:
choose an agent in a discovery or notification surface, then inspect that
agent in the session already placed beside it.

## Eligibility

The Workbench derives normalized pane bounds from its persisted split tree.
A target must meet every condition below:

1. It is the active tab of a different pane, so it is visible.
2. It is an `agent-session` surface.
3. Its pane shares a non-zero edge with the invoking pane.
4. The Workbench is not zoomed to one pane.

If more than one eligible pane shares an edge, Wardian selects the one with
the longest shared boundary. A tie remains deterministic in split-tree order.

## Fallback and Safety

If there is no eligible target, Wardian uses the existing resource-aware
focus-or-open behavior. A contextual open does not alter the pane tree, create
a duplicate presentation, or issue any agent lifecycle command.

Rebinding uses the same guarded close transaction as an explicit Agent Session
rebind. If that transaction is cancelled or becomes stale, the existing
presentation remains unchanged and Wardian does not create a replacement tab.

## Scope Boundaries

- The right roster keeps its explicit **Open** and **Open to Side** semantics.
- Files and artifacts are excluded because replacing a visible editor could
  conflict with dirty-buffer intent.
- Workflow-to-agent targeting is deferred until workflows expose a stable,
  unambiguous execution inspection target.

## Verification

- Unit tests cover adjacent-pane detection, inactive-tab exclusion, and
  deterministic selection among multiple neighbors.
- Navigation tests cover rebind-without-new-tab, ordinary fallback, and the
  zoomed-pane fallback.
- App integration verifies a Graph action retargets an adjoining Agent Session
  without triggering an agent lifecycle operation.
