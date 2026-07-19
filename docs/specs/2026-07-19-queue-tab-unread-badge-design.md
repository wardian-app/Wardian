# Queue Tab Unread Badge

## Goal

Restore the Queue tab's notification badge after the migration to Workbench surface tabs, preserving the previous numeric unread-count behavior.

## Behavior

- The Queue surface has no badge when all queue items are read or the queue is empty.
- The Queue surface shows one accent badge when unread items exist.
- The badge displays the exact unread count through `9`, then `9+` for larger counts.
- The count updates when queue items load, arrive, are marked read, are cleared, or are dismissed.
- The badge's accessible label describes the unread count; the existing Workbench tab title remains `Queue`.

## Architecture

The Queue surface definition owns the badge derivation because Workbench presentation metadata is the canonical source for tab titles, icons, commands, and badges. It reads the unread count from `useQueueStore` and subscribes the surface registry to queue-store updates through the existing `presentation_subscribe` mechanism. No queue persistence or event-generation behavior changes.

The shared `SurfaceBadge` DTO gains an optional short display value. `WorkbenchTab` renders that value when present while retaining dot-only rendering for existing dirty, attention, and recovery badges. Queue uses a stable `unread` badge identifier and the themed accent styling already used for positive Workbench badges.

## Verification

- Unit coverage proves Queue presentation metadata emits no badge for zero unread items, emits the count for one unread item, and caps the display at `9+`.
- Workbench tab coverage proves a badge display value is rendered without changing the tab title.
- Workbench host coverage proves a subscribed Queue-store update refreshes the visible Queue tab badge.
- Run the focused tests, then the frontend lint, full unit test, and build commands required by the repository checklist.
- Capture feature-specific screenshot evidence of a Queue tab with unread items for the frontend PR description.
