# Garden automation population

- **Date:** 2026-09-10
- **Status:** Implemented
- **Precedence:** Extends the Garden crisp-rendering spec. This decision replaces its broad current/recent automation population rule for one-off runs.

## Decision

Garden represents durable routine geography and work happening now. Assigned
routine definitions remain situated around their workspaces and agents. An
unscheduled automation run appears while it is running or awaiting approval,
then leaves the spatial projection when it completes or fails.

Recency alone must never promote a terminal one-off run into a Garden map unit.
Its durable evidence remains available in Automation Monitor and Observe. An
already opened historical run can remain mounted as focused navigation state,
without adding its trail back to the canvas. Recent scheduled evidence stays
inside the routine it belongs to.

Filter terminal one-off summaries before loading their run details or invocation
records. This keeps large review sessions from spending rendering space and
refresh work on dozens of historical trails.

## Acceptance evidence

Project 80 recent completed one-off runs alongside an active scheduled routine.
The Garden must render only the routine, issue no detail or invocation reads for
the 80 terminal runs, and continue to expose the routine in an agent composition.
Unit coverage must also preserve active unscheduled runs and explicitly retained
historical evidence.
