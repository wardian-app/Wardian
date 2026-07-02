# Graph

The Graph view is the control surface for the communication topology. Use it to build and inspect your agent network: see how agents cluster by team or manual connection, create/delete edges, and understand the default communication boundaries that shape CLI behavior and agent visibility.

![Wardian Graph view showing status-colored agent nodes, relationship lenses, and the inspector](../assets/screenshots/graph/graph-view.png)

## What the Graph Shows

Each node is an agent. Nodes use status colors (Idle=Emerald, Processing=Cyan, Error=Red, etc.) and recency glows. Edges represent relationships in the communication topology:

**Edge types and textures:**
- **Solid edges**: manual connections (you created them by dragging in the graph).
- **Dotted edges**: rule-derived edges from team membership (team-clique rule).
- **Sparse dashed edges**: unmapped ghost connections (recent communication traffic between agents with no manual edge or team connection).

**Communication state (color + motion):**
- **Cyan + directed particles**: ongoing or recent active conversation.
- **Light cyan fading**: recent activity within the last hour (fades over time).
- **Dim gray**: dormant (no recent activity).

Particles flow in message direction; during a pending ask, the stream drifts toward the agent that owes the reply — direction is the pending-ask indicator.

## Topology & Workspace Fallback

An agent with **no manual edges and no team membership** automatically sees its workspace-mates (workspace-fallback rule). This ensures fresh agents aren't isolated. The moment you draw an agent's first manual edge or add it to a team, workspace-fallback disengages — its neighbors become exactly what the graph shows.

Each neighbor is labeled with its origin: `manual`, `rule:team-clique`, or (in the inspector) `rule:workspace-fallback` / `ghost`.

## Editing: Create and Delete Edges

**Create a connection:**
- Enable **Connect mode** in the toolbar, then drag from agent A to agent B to draw a manual edge.
- Or use **Add connection…** in the inspector's neighbors panel to pick an agent from a searchable list.
- The edge appears immediately and is saved to `<WARDIAN_HOME>/topology.json`.

**Delete a connection:**
- Click a manual edge to select it, then press Delete — or use the disconnect (×) button on the edge's row in the inspector.
- Only manual edges are deletable; rule-derived edges are labeled "managed by team …" and are edited through their source (e.g., team membership).

**Ghost edges (unmapped traffic):**
- Recent communication between agents with no topology connection appears as a faint sparse-dashed edge, and the inspector's neighbors panel lists the pair with an **Unmapped** badge.
- **Formalize**: write a manual edge to connect them.
- **Ignore**: add the pair to `ignored_pairs` so the suggestion stops appearing.

## Inspector and Actions

Select any node to open the inspector with:
- Agent identity, current status, workspace, and telemetry.
- **Neighbors panel**: all agents you see through the topology (manual, team, workspace fallback), each tagged with its origin reason.
- **Add connection…**: searchable picker to create new manual edges.
- Right-click to access the same context menu as the roster and other views.

## Topology Source of Truth

The graph is backed by `<WARDIAN_HOME>/topology.json` (default: `~/.wardian/topology.json`), an inspectable JSON file:

```json
{
  "version": 1,
  "edges": [
    { "a": "agent-uuid-1", "b": "agent-uuid-2", "created_at": "2026-07-02T14:30:00Z" }
  ],
  "ignored_pairs": [
    { "a": "agent-uuid-3", "b": "agent-uuid-4" }
  ]
}
```

You can edit this file directly (the app reloads on changes), or use the UI. Edges are undirected and canonicalized (`a < b` lexicographically).

## Legacy Lenses

The three legacy relationship overlays (Same Team, Shared Workspace, Same Worktree) remain as read-only lenses in the toolbar, off by default. Toggle them to see derived signals without affecting the topology.
