# Mobile PWA Watchlist

## Purpose

Wardian's remote PWA currently shows agents as standalone cards in the order returned by the remote roster. That misses the desktop watchlist model, especially teams, so a phone cannot monitor a swarm using the same organization the desktop user configured.

This PR reconstructs the PWA first screen as a monitoring-only watchlist view inspired by TradingView's compact mobile watchlists. The list should be clean, dense, and fast to scan. The only row action is opening the agent detail view; all operational actions remain in the agent view.

## Scope

In scope:

- Add a read-only remote watchlist endpoint.
- Load desktop watchlists, teams, and watchlist preferences in the PWA.
- Render a compact mobile watchlist with team ordering.
- Remember the active mobile watchlist locally.
- Add a bottom navigation bar with Watchlist as the real tab and placeholder tabs for Workflows, Queue, Graph, and Library.
- Remove the always-visible broadcast command bar from the watchlist first screen.

Out of scope:

- Creating, renaming, deleting, or reordering watchlists from mobile.
- Mutating team membership from mobile.
- Sending prompts or lifecycle actions from the watchlist.
- Syncing mobile collapse toggles back to desktop preferences.
- Implementing real Workflows, Queue, Graph, or Library mobile tabs.

## Remote Data

Add:

```http
GET /remote/api/watchlists
```

The endpoint returns:

```ts
{
  watchlists: Watchlist[];
  teams: AgentTeam[];
  prefs: WatchlistPrefs | null;
}
```

The gateway reads:

- `<wardian-home>/watchlists/index.json`
- `<wardian-home>/watchlists/prefs.json`

The endpoint is read-only and uses the existing authenticated remote request boundary. Missing files return an empty watchlist state and `prefs: null` rather than failing the whole remote shell.

## Ordering Rules

The PWA reuses the same frontend watchlist normalization and display utilities as the desktop roster:

- All Agents preserves the backend roster order for standalone agents.
- Teams render as blocks the first time a team member appears in the roster.
- Team members render in `team.agentIds` order.
- Custom watchlists render `entries` order.
- Team entries expand to ordered team members.
- Missing agent ids are skipped.
- Empty team blocks are not rendered.

The remote UI can adapt `RemoteAgentSummary` into the minimum agent shape required by the shared watchlist utilities. This keeps ordering semantics shared without adopting the desktop watchlist component's editing, drag, context-menu, column, and sidebar behavior.

## Mobile UI

The PWA first screen becomes a dedicated watchlist view.

Header:

- Compact `Wardian` title.
- Current watchlist name.
- Refresh icon button.

Watchlist selector:

- Horizontal selector with `All` and user watchlists.
- Compact, pill-like styling.
- No mobile list management controls in this PR.

Agent list:

- Dense rows instead of cards.
- Each row is a full-width button that opens the existing agent detail view.
- Left side shows status dot, agent name, class, and provider.
- Right side shows status label.
- Latest text can appear as muted secondary text when available.
- Workspace is secondary and should not dominate the row.
- Rows do not expose inline actions.

Team sections:

- Header shows team name, visible member count, and a chevron.
- Teams are expanded by default unless desktop prefs include the team id in `collapsed_team_ids`.
- Tapping the chevron toggles local collapse state.
- Tapping a member row opens that agent.
- Tapping the team header outside the chevron does not perform an operational action.

Bottom navigation:

- Fixed bottom bar.
- Real tab: Watchlist.
- Placeholder tabs: Workflows, Queue, Graph, Library.
- Placeholder tabs show a compact unavailable panel and do not add feature behavior.

## State

Extend the remote store with:

- `watchlists`
- `teams`
- `watchlistPrefs`
- `activeWatchlistId`
- `activeRemoteTab`
- local collapsed team ids for mobile overrides

Startup behavior:

1. Load the authenticated remote session.
2. Load agents and watchlist state.
3. Use the locally stored active watchlist id if it still exists.
4. Fall back to `all` when no stored id exists or the stored id is stale.
5. Connect the status stream.

Refresh behavior:

- Manual refresh reloads agents, workflows compatibility data if still present, and watchlist state.
- Status stream agent updates replace the live agent list only.
- Watchlist and team changes are picked up on manual refresh or page reload.

Collapse behavior:

- Initial collapsed team ids come from returned desktop prefs.
- Mobile toggles are local-only for this PR.
- Mobile collapse state is not written to the desktop watchlist prefs file.

## Failure Handling

- If `/remote/api/watchlists` returns `404`, the PWA falls back to All Agents without teams. This keeps older gateway builds usable.
- If watchlist JSON is malformed, the backend returns an empty normalized state.
- If prefs JSON is missing or malformed, the backend returns `prefs: null`.
- Agent detail routing remains unchanged when an active agent disappears from the status stream.

## Testing

Frontend unit tests:

- Remote store loads watchlists, teams, and prefs from the remote endpoint.
- Missing remote watchlist endpoint falls back to All Agents.
- Stored active watchlist id is restored when valid.
- Stale active watchlist id falls back to `all`.
- All Agents renders team blocks in team member order.
- Custom watchlist renders team entries and standalone entries in watchlist order.
- Team chevron collapses and expands locally.
- Agent row click opens the existing detail view.
- Bottom navigation shows Watchlist plus placeholder tabs.

Rust tests:

- Remote watchlist endpoint returns normalized watchlists, teams, and prefs.
- Missing files return empty watchlist state and `prefs: null`.
- Malformed persisted JSON does not crash the endpoint.
- Endpoint requires the same remote session boundary as other roster reads.

Browser E2E:

- Remote PWA smoke renders a compact watchlist list rather than card grid.
- Seeded remote watchlist team state renders agents in team order.
- Bottom bar placeholder tabs are visible and switch to placeholder panels.

## Documentation And PR Evidence

Update remote-control or watchlist guide text to mention that the PWA watchlist mirrors desktop team/watchlist organization for monitoring.

Because this is a frontend behavior change, the implementation PR must include a feature-specific screenshot under `e2e/screenshots/<feature>/<timestamp>/` and embed an uploaded HTTPS image in the PR description.
