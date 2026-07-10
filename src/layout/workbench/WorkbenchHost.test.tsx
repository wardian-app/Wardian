import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { createWorkbenchStore } from "../../features/workbench/useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../../features/workbench/workbenchTestUtils";
import { WorkbenchHost, workbenchEdgeDropCommands } from "./WorkbenchHost";

describe("WorkbenchHost", () => {
  it("keeps group zoom runtime-only", () => {
    const initialDocument = makeSingleGroupDocument();
    const store = createWorkbenchStore({ initial_document: initialDocument });
    const documentBefore = store.getState().document;

    render(<WorkbenchHost store={store} />);
    fireEvent.click(screen.getByRole("button", { name: "Toggle zoom for group-1" }));

    expect(store.getState().zoomed_group_id).toBe("group-1");
    expect(store.getState().document).toBe(documentBefore);
    expect(screen.getByTestId("workbench-host")).toHaveAttribute(
      "data-zoomed-group-id",
      "group-1",
    );
  });

  it("translates edge drops into one canonical split-and-move transaction", () => {
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const initial = makeSingleGroupDocument([surface]);
    let nextId = 0;
    const commands = workbenchEdgeDropCommands(
      "surface-1",
      "group-1",
      "left",
      (kind) => `${kind}-new-${++nextId}`,
    );

    const store = createWorkbenchStore({ initial_document: initial });
    const applied = store.getState().apply_commands(commands);
    if (!applied.accepted) throw new Error(applied.errors[0]?.message ?? "command rejected");
    const result = store.getState().document;

    expect(commands).toEqual([
      {
        type: "split_group",
        group_id: "group-1",
        new_group_id: "group-new-1",
        node_id: "node-new-2",
        direction: "horizontal",
        placement: "before",
      },
      {
        type: "move_surface",
        surface_id: "surface-1",
        group_id: "group-new-1",
        index: 0,
      },
    ]);
    expect(result.root).toEqual({
      kind: "split",
      node_id: "node-new-2",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "group", group_id: "group-new-1" },
      second: { kind: "group", group_id: "group-1" },
    });
  });
});
