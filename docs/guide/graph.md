# Graph

The Graph view shows active agents as a relationship map. Use it when you need to see how agents cluster by team, workspace, or worktree without opening every terminal pane.

![Wardian Graph view showing status-colored agent nodes, relationship lenses, and the inspector](../assets/screenshots/graph/graph-view.png)

## What the Graph Shows

- Each node is an agent. Node color uses the same status colors as the rest of Wardian.
- Recency appears as a glow around the node, while node size stays constant.
- Edges are explicit relationship edges only:
  - Same Team
  - Shared Workspace
  - Same Worktree

Watchlist membership does not create a graph edge. Communication-specific edges require explicit pairwise communication data and are not included in this first graph view.

## Relationship Lenses

Use the lens buttons at the top of the view to show or hide relationship types. The lens controls stay centered over the graph canvas, and the inspector can be hidden from its top-right button when you want more canvas space.

## Inspector and Actions

Selecting a node opens the inspector with the agent's identity, current status, workspace path, telemetry, and visible relationships. Right-click graph nodes or relationship rows to use the same agent context menu actions available from the roster and other main views.
