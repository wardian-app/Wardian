import { describe, expect, it } from "vitest";

import type { WorkbenchDocumentV1, WorkbenchGroupV1, WorkbenchNodeV1, WorkbenchSurfaceV1 } from "../../types";
import { findAdjacentActiveSurface } from "./adjacentSurfaceTargeting";
import { DEFAULT_TEST_SHELL, makeSurface } from "./workbenchTestUtils";

function documentWithLayout(
  root: WorkbenchNodeV1,
  groups: Record<string, WorkbenchGroupV1>,
  surfaces: WorkbenchSurfaceV1[],
  activeGroupId: string,
): WorkbenchDocumentV1 {
  return {
    schema_version: 1,
    revision: 0,
    saved_at: "1970-01-01T00:00:00.000Z",
    root,
    groups,
    surfaces: Object.fromEntries(surfaces.map((surface) => [surface.surface_id, surface])),
    active_group_id: activeGroupId,
    recently_closed: [],
    shell: { ...DEFAULT_TEST_SHELL },
  };
}

describe("findAdjacentActiveSurface", () => {
  it("finds the active matching surface in a pane that shares an edge", () => {
    const graph = makeSurface("graph", { surface_type: "graph" });
    const agent = makeSurface("agent", {
      surface_type: "agent-session",
      resource_key: "agent-1",
    });
    const document = documentWithLayout(
      {
        kind: "split",
        node_id: "split-root",
        direction: "horizontal",
        ratio: 0.5,
        first: { kind: "group", group_id: "graph-group" },
        second: { kind: "group", group_id: "agent-group" },
      },
      {
        "graph-group": {
          group_id: "graph-group",
          surface_ids: [graph.surface_id],
          active_surface_id: graph.surface_id,
        },
        "agent-group": {
          group_id: "agent-group",
          surface_ids: [agent.surface_id],
          active_surface_id: agent.surface_id,
        },
      },
      [graph, agent],
      "graph-group",
    );

    expect(findAdjacentActiveSurface(document, graph.surface_id, "agent-session"))
      .toBe(agent.surface_id);
  });

  it("does not target a matching tab that is inactive in its pane", () => {
    const graph = makeSurface("graph", { surface_type: "graph" });
    const agent = makeSurface("agent", {
      surface_type: "agent-session",
      resource_key: "agent-1",
    });
    const notes = makeSurface("notes", { surface_type: "notes" });
    const document = documentWithLayout(
      {
        kind: "split",
        node_id: "split-root",
        direction: "horizontal",
        ratio: 0.5,
        first: { kind: "group", group_id: "graph-group" },
        second: { kind: "group", group_id: "mixed-group" },
      },
      {
        "graph-group": {
          group_id: "graph-group",
          surface_ids: [graph.surface_id],
          active_surface_id: graph.surface_id,
        },
        "mixed-group": {
          group_id: "mixed-group",
          surface_ids: [agent.surface_id, notes.surface_id],
          active_surface_id: notes.surface_id,
        },
      },
      [graph, agent, notes],
      "graph-group",
    );

    expect(findAdjacentActiveSurface(document, graph.surface_id, "agent-session"))
      .toBeUndefined();
  });

  it("prefers the adjacent surface sharing the longest edge", () => {
    const graph = makeSurface("graph", { surface_type: "graph" });
    const topAgent = makeSurface("top-agent", {
      surface_type: "agent-session",
      resource_key: "agent-top",
    });
    const bottomAgent = makeSurface("bottom-agent", {
      surface_type: "agent-session",
      resource_key: "agent-bottom",
    });
    const document = documentWithLayout(
      {
        kind: "split",
        node_id: "split-root",
        direction: "horizontal",
        ratio: 0.5,
        first: { kind: "group", group_id: "graph-group" },
        second: {
          kind: "split",
          node_id: "split-right",
          direction: "vertical",
          ratio: 0.7,
          first: { kind: "group", group_id: "top-agent-group" },
          second: { kind: "group", group_id: "bottom-agent-group" },
        },
      },
      {
        "graph-group": {
          group_id: "graph-group",
          surface_ids: [graph.surface_id],
          active_surface_id: graph.surface_id,
        },
        "top-agent-group": {
          group_id: "top-agent-group",
          surface_ids: [topAgent.surface_id],
          active_surface_id: topAgent.surface_id,
        },
        "bottom-agent-group": {
          group_id: "bottom-agent-group",
          surface_ids: [bottomAgent.surface_id],
          active_surface_id: bottomAgent.surface_id,
        },
      },
      [graph, topAgent, bottomAgent],
      "graph-group",
    );

    expect(findAdjacentActiveSurface(document, graph.surface_id, "agent-session"))
      .toBe(topAgent.surface_id);
  });
});
