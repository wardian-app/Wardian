import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  DockviewEvaluationHarness,
  createDockviewProofModel,
  moveProofSurface,
  serializeDockviewProofModel,
} from "./DockviewEvaluationHarness";

describe("DockviewEvaluationHarness", () => {
  it("drives four groups and 20 tabs from Wardian-owned proof state", async () => {
    render(<DockviewEvaluationHarness />);

    await waitFor(() => {
      expect(screen.getAllByTestId(/^proof-group-/)).toHaveLength(4);
    });

    expect(screen.getAllByRole("tab")).toHaveLength(20);
    expect(screen.getByTestId("workbench-proof")).toHaveAttribute(
      "data-layout-source",
      "wardian-model",
    );

    const model = JSON.parse(screen.getByTestId("proof-model").textContent ?? "null") as {
      groups: Array<{ group_id: string; surface_ids: string[] }>;
    };
    expect(model.groups.map((group) => group.group_id)).toEqual([
      "proof-group-1",
      "proof-group-2",
      "proof-group-3",
      "proof-group-4",
    ]);
    expect(model.groups.flatMap((group) => group.surface_ids)).toHaveLength(20);
  });

  it("moves a tab without remounting its keyed child", async () => {
    render(<DockviewEvaluationHarness />);

    const owner = await screen.findByTestId("proof-surface-terminal-owner");
    expect(owner).toHaveAttribute("data-mount-count", "1");

    fireEvent.click(screen.getByRole("button", { name: "Move terminal owner to group 2" }));

    await waitFor(() => {
      expect(screen.getByTestId("proof-surface-terminal-owner")).toBe(owner);
    });
    expect(owner).toHaveAttribute("data-mount-count", "1");

    const model = JSON.parse(screen.getByTestId("proof-model").textContent ?? "null") as {
      groups: Array<{ group_id: string; surface_ids: string[] }>;
    };
    expect(
      model.groups.find((group) => group.group_id === "proof-group-2")?.surface_ids,
    ).toContain("terminal-owner");
  });

  it("zooms and unzooms without mutating serialized proof state", async () => {
    render(<DockviewEvaluationHarness />);

    await screen.findByTestId("proof-surface-terminal-owner");
    const before = screen.getByTestId("proof-model").textContent;
    const zoomButton = screen.getByRole("button", { name: "Toggle group 1 zoom" });

    fireEvent.click(zoomButton);
    expect(screen.getByTestId("workbench-proof")).toHaveAttribute(
      "data-zoomed-group-id",
      "proof-group-1",
    );
    expect(screen.getByTestId("proof-model")).toHaveTextContent(before ?? "");

    fireEvent.click(zoomButton);
    expect(screen.getByTestId("workbench-proof")).toHaveAttribute(
      "data-zoomed-group-id",
      "none",
    );
    expect(screen.getByTestId("proof-model")).toHaveTextContent(before ?? "");
  });

  it("restores from the plain proof model without Dockview JSON", async () => {
    const restored = moveProofSurface(
      createDockviewProofModel(),
      "terminal-owner",
      "proof-group-4",
    );

    render(<DockviewEvaluationHarness initialModel={restored} />);

    await waitFor(() => {
      expect(screen.getAllByRole("tab")).toHaveLength(20);
    });

    expect(screen.getByTestId("proof-model")).toHaveTextContent(
      serializeDockviewProofModel(restored),
    );
    expect(screen.getByTestId("workbench-proof")).toHaveAttribute(
      "data-layout-source",
      "wardian-model",
    );
    expect(screen.getByTestId("workbench-proof")).not.toHaveAttribute(
      "data-layout-source",
      "dockview-json",
    );
  });
});
