# Communication Interaction Graph

## Summary

Wardian will replace the Graph tab placeholder with a first-version operational communication and interaction graph. The view is a frontend-only projection over existing app state. It does not introduce a backend graph model, lifecycle authority, communication event store, or new source of truth.

The visual model resembles an Obsidian-style network map: agents are small status-colored nodes, relationships are subdued edges, and detail lives in an inspector. The graph is for operational status first and relationship discovery second.

## Goals

- Show all agents in the active graph scope as compact orb nodes.
- Use agent status as the primary visual encoding.
- Project existing teams, active watchlist scope, workspace/worktree relationships, and recent activity into a network map.
- Keep graph interactions consistent with Wardian's existing selection and context-menu behavior.
- Preserve Grid, Dashboard, and Watchlist as the primary places for dense operations and terminal detail.
- Keep v1 lightweight enough for many agents by using a renderer suited to network visualization.

## Non-Goals

- No backend command or persisted graph schema.
- No inferred agent-to-agent communication from query counts or last-query timestamps.
- No workflow DAG, run trace, or artifact graph integration.
- No editing teams, worktrees, workflows, or graph layout from the graph.
- No continuous force simulation running in the browser.
- No saved manual layouts, graph export, or WebGL-scale tuning beyond the initial renderer choice.

## Renderer Decision

Use `sigma.js` for the graph canvas and WebGL rendering layer. Wardian already uses `@xyflow/react` for workflow editing, but React Flow is optimized for node-editor surfaces where nodes are React components. The communication graph needs an Obsidian-like network of many lightweight orb nodes and edges. Sigma's WebGL rendering model is the better fit for that interaction model and future scale.

React Flow remains the workflow builder renderer. The communication graph does not share the workflow editor's node-card interaction model.

## Layout Model

V1 uses deterministic static layout rather than continuous browser physics.

- Team membership creates loose visual clusters.
- Agents without a team are placed in a shared ungrouped region.
- Active watchlist selection filters visible agents and appears as a scope label; it does not create clusters or relationships.
- Shared workspace or worktree relationships add subdued edges between agent nodes.
- Recent activity is shown as a node halo or intensity treatment, not as a directional communication edge.
- The layout is stable across renders when the underlying agents and relationships have not changed.

The implementation starts with a deterministic radial or cluster layout. A later version can add a one-time force layout pass or worker-backed layout if graph readability requires it.

## Data Projection

Create a pure frontend projection utility that maps existing app state into a graph model.

Inputs:

- `AgentConfig[]` for identity, display name, class, provider, workspace, and worktree metadata.
- `Record<string, AgentTelemetry>` for current status, query count, uptime, and resource usage.
- `AgentTeam[]` for clustering and `Watchlist | null` for visible agent scope.
- `AgentInteractions` for last-query recency.
- Selected agent IDs for highlight state.

Outputs:

- `GraphNode[]`: agent nodes with ID, label, status, provider/class metadata, coordinates, cluster ID, and recency attributes.
- `GraphEdge[]`: aggregated undirected relationship edges with source, target, weight, and reason list.
- `GraphCluster[]`: cluster metadata for team labels and inspector grouping.

Relationship reasons are explicit and conservative:

- `same_team`
- `shared_workspace`
- `same_worktree`

`agentInteractions` must not imply a communication edge. It only supports a recent-activity visual treatment for the queried agent.

## Graph Scope

The graph computes nodes and edges only for visible agents.

- All Agents scope uses the same ordering as the full roster.
- Active watchlist scope uses `getAgentsForList(agents, activeList, teams)` so team entries expand to their member agents while preserving existing watchlist/team ordering.
- Watchlist membership affects visibility and scope labeling only.
- Team membership affects clustering and `same_team` edges whether the graph is scoped to All Agents or a watchlist.
- Edges never target hidden agents. If only one member of a team or workspace is visible, that relationship produces no rendered edge.

## Edge Rules

The graph creates an undirected edge only when two visible agents share at least one explicit relationship reason. One rendered edge can aggregate multiple reasons.

Create an edge for these relationships:

- `same_team`: both agents are members of the same `AgentTeam`.
- `shared_workspace`: both agents have the same normalized non-empty `folder`.
- `same_worktree`: both agents have the same normalized non-empty `git_worktree_folder`.

Do not create edges for these signals:

- Same watchlist membership. Watchlists control graph scope and ordering; they do not imply a relationship.
- Recent activity in `AgentInteractions`. Recent activity affects node treatment only.
- Similar status, provider, model, class, or query count.
- Inferred communication from terminal output, current thought text, telemetry, or last-query time.

Normalization rules:

- Trim whitespace before comparing string fields.
- Replace backslashes with `/`.
- Collapse duplicate separators in the path body without changing a leading UNC `//` prefix.
- Remove trailing separators except for roots such as `/`, `C:/`, and `//server/share`.
- Lowercase Windows-style drive paths and UNC paths for comparison.
- Ignore empty strings and missing values.
- Do not query the filesystem, resolve symlinks, resolve junctions, canonicalize case from disk, or expand environment variables in v1.
- Aggregate duplicate reasons into one edge between the same unordered pair of agents.
- Compute edges only among currently visible graph agents.

Examples:

| Agents share | Edge? | Reason |
|---|---:|---|
| Same `AgentTeam` | Yes | `same_team` |
| Same normalized `folder` | Yes | `shared_workspace` |
| Same normalized `git_worktree_folder` | Yes | `same_worktree` |
| Same normalized `git_worktree_source` only | No | Source repository alone does not mean same worktree |
| Same watchlist only | No | Watchlist is scope, not relationship |
| Same status, provider, model, class, or query count | No | Similarity is not interaction |
| Recent activity only | No | Recent activity is node treatment only |
| Hidden related agent | No | Edges are visible-agent-only |

## UX

The Graph tab becomes a full-height operational map. Garden remains a placeholder.

Main surface:

- Sigma canvas with panning, zooming, fit/reset view, and stable node positions.
- Lens toggles for relationship types: same team, same workspace, and same worktree.
- Scope indicator for current watchlist versus all agents.

Node treatment:

- Agents are orbs, not cards.
- Status colors follow Wardian conventions: emerald idle, cyan processing, amber action required, gray off, red error.
- Agent node size is constant; status does not affect size.
- Selected and hovered nodes highlight adjacent edges.
- Recent activity adds a restrained glow or halo, not a size change.

Inspector:

- Opens or updates when a node is selected.
- Shows agent name, class, provider, current status summary from existing telemetry/state, workspace/worktree path, telemetry summary, and relationship reasons grouped by neighboring agent.
- Supports the same right-click context-menu behavior as the Watchlist and existing main views where applicable, including rename, query, delete, add to list, and remove from list.
- Provides an "open in Grid" path that switches to Grid and focuses the agent.

Interaction:

- Click node: select the agent and update `selectedAgentIds`.
- Hover node or edge: show relationship tooltip and highlight related elements.
- Right-click node or inspector agent row: open the existing agent context menu.
- Double-click node or inspector action: open the agent in Grid.
- Empty or relationship-free states still show agent status nodes and explain that no relationships are visible under the current lens/scope.

## Implementation Shape

Add these frontend modules:

- `src/features/graph/graphProjection.ts`
- `src/features/graph/GraphCanvas.tsx`
- `src/views/GraphView.tsx`

Update:

- `src/views/App.tsx` to render `GraphView` for `viewMode === "graph"` and pass existing state/handlers.
- `src/styles/App.css` or a graph-specific imported stylesheet for themed graph controls and inspector layout.
- `package.json` and lockfile to add Sigma and graphology dependencies if they are not already present.

Keep all graph business rules in `graphProjection.ts`. `GraphCanvas.tsx` adapts the projected model to Sigma and binds pointer events. `GraphView.tsx` owns toolbar, legend, inspector, and context-menu integration.

## Testing

Unit tests:

- Projection creates one node per visible agent.
- All Agents and active watchlist scopes produce the expected visible agent set.
- Watchlist co-membership does not create edges.
- Team clusters preserve team membership and order.
- Team membership creates `same_team` edges only between visible team members.
- Shared workspace/worktree reasons aggregate into a single edge between an unordered pair of agents.
- Same worktree is computed from `git_worktree_folder`; `git_worktree_source` alone does not create a relationship.
- Path normalization treats `C:\repo`, `C:/repo`, and `C:/repo/` as the same Windows-style path.
- Empty, missing, or whitespace-only path fields do not create edges.
- Edges are not created to hidden agents.
- Recent interaction affects node recency metadata but does not create communication edges.
- Recent interaction renders as a halo/glow while node size remains constant.
- Unsupported inference is omitted.

Component tests:

- Graph view renders with agents and relationship lenses.
- Empty graph state is handled.
- Lens toggles alter visible edge types.
- Selecting a node opens inspector details.
- Inspector/right-click exposes existing context-menu actions.

Verification:

Targeted frontend verification:

- Run `npm run lint`.
- Run `npm run test`.
- Run `npm run build`.

Final PR verification:

- Follow the repo pre-commit checklist, including backend checks, before requesting merge or opening a PR.
- If Rust code remains untouched, note that the graph implementation is frontend-only while still reporting whether backend checks were run.
- Capture feature-specific screenshot evidence under `e2e/screenshots/graph/<timestamp>/` for PR documentation.

## Acceptance Criteria

- The Graph tab renders a real operational graph view instead of the placeholder.
- Agent nodes use status color as the primary visual cue.
- The graph is scoped to the active watchlist or all agents consistently with existing watchlist state.
- Team and workspace/worktree relationships are visible or toggleable.
- Edges are aggregated and reveal relationship reasons on hover or selection.
- Node selection updates the inspector and selected-agent state.
- Right-click behavior is available from graph nodes or the graph inspector with existing agent context-menu actions.
- The graph does not infer direct communication without explicit event data.
- The implementation is frontend-only unless a later requirement proves a backend command is necessary.
