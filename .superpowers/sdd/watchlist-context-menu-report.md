# Watchlist context menu consolidation report

Status: DONE

## Implemented

- Added optional single-agent `onOpen` and `onOpenToSide` primary actions to `AgentContextMenu`.
- Rendered the primary actions at the top of the shared menu with one divider before the existing management actions.
- Removed the watchlist's separate `agent-open-context-menu` wrapper and vertical offset.
- Routed watchlist navigation callbacks through the existing `agent-context-menu` and updated App integration assertions.
- Added shared-component coverage for optional action presence, ordering, invocation, close behavior, and absence.

## TDD evidence

### RED

`npm run test -- src/components/AgentContextMenu.test.tsx src/layout/watchlist/AgentWatchlist.test.tsx src/views/App.test.tsx`

- Failed as expected: 3 failed, 129 passed.
- `AgentContextMenu` started with Rename/Query instead of Open/Open to Side.
- `AgentWatchlist` rendered two `.context-menu` elements instead of one.
- App could not find Open in the unified `agent-context-menu`.

### GREEN

`npm run test -- src/components/AgentContextMenu.test.tsx src/layout/watchlist/AgentWatchlist.test.tsx src/views/App.test.tsx`

- Passed: 3 files, 132 tests.

## Verification

- `npm run lint` - passed (`tsc --noEmit`).
- `git diff --check` - passed.
- `rg -n "agent-open-context-menu" src` - no matches.
- Scope review confirmed only the five requested source/test files and this report were changed by this task. The two pre-existing unstaged docs whitespace edits were not touched.

## Self-review

- Navigation actions receive the context menu's `agentId`, await sync or async callbacks, and close through the shared `onClose` path.
- Primary actions are gated to non-team, non-bulk menus; callers without the optional props retain their previous menu shape.
- The watchlist now anchors one shared menu at the original pointer coordinates with no second wrapper or y-offset.
- Tests assert one `.context-menu`, both navigation callbacks, management-action co-location, action ordering, close behavior, and absent optional actions.

## Concerns

- No blockers. Bulk selections intentionally omit these single-agent primary actions, matching the brief's single-agent boundary.
- The live Wardian client was not restarted or modified.

## Commit

- `feat(watchlist): unify agent context menu`
