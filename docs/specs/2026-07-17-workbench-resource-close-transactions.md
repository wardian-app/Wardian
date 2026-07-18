# Workbench Resource Close Transactions

## Decision

All destructive Workbench navigation uses the pure close transaction
coordinator. Sequential per-surface `can_close` callbacks are removed.

The navigation service captures one immutable Workbench document, transaction
version, and complete intended closing-surface set. The surface registry maps
those presentations to resource-owned adapters. An adapter reports canonical
resource identity, resource generation, presentation membership, dirty state,
and deferred choice effects without importing React, Monaco, or another view.

## Transaction Order

1. Observe every resource touched by the complete closing set.
2. Prepare each final-closing dirty resource once and collect all choices.
3. Cancel with no effects when any choice cancels or preparation fails.
4. Revalidate the live transaction version plus exact resource identity,
   generation, and presentation membership.
5. Run every requested Save in deterministic closing-set order.
6. Compare-and-apply the layout mutation once.
7. Run Discard and in-memory release only after the layout commit succeeds.

A duplicate presentation or resource rebind opened while a choice is pending
makes the preparation stale before effects. A remaining presentation prevents
resource preparation because the resource is not final-closing. A successful
earlier Save remains valid if a later Save fails, but layout does not commit and
no Discard runs.

## Surface Adapters

- Library uses presentation-keyed resources matching its mounted editor bridge.
- Workflows uses one shared builder resource and its edit revision as generation.
- Files accepts a narrow injected resource adapter. The default adapter reports
  clean/no effects until the Files editor controller supplies dirty buffers,
  saving, discard, and recovery cleanup.

Missing or malformed adapters fail closed for `confirm_if_dirty` surfaces.
`close_view` surfaces do not participate in resource preparation.

## Scope

This decision covers canonicalize, resource rebind, surface reset, surface
close, group close, and Workbench reset. It does not add Files editor buffers,
native saving, recovery state, or recovery UI.
