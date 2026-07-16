import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { DeepReadonly } from "../../features/workbench/useWorkbenchStore";
import { makeSingleGroupDocument, makeSurface } from "../../features/workbench/workbenchTestUtils";
import type { WorkbenchSurfaceV1 } from "../../types";

type PointerDragIdentity = {
  surface_id: string;
  source_group_id: string;
  pointer_id: number;
};

type MockWorkbenchTabProps = {
  surface: DeepReadonly<WorkbenchSurfaceV1>;
  group_id: string;
  on_pointer_drag_start?: (identity: PointerDragIdentity) => void;
  on_pointer_drag_end?: (identity: PointerDragIdentity) => void;
};

vi.mock("./WorkbenchTab", () => ({
  WorkbenchTab: ({
    surface,
    group_id,
    on_pointer_drag_start: onPointerDragStart,
    on_pointer_drag_end: onPointerDragEnd,
  }: MockWorkbenchTabProps) => {
    const pointerId = surface.surface_id === "surface-1" ? 7 : 8;
    const identity = {
      surface_id: surface.surface_id,
      source_group_id: group_id,
      pointer_id: pointerId,
    };
    return (
      <span>
        <button type="button" onClick={() => onPointerDragStart?.(identity)}>
          Start {surface.surface_id}
        </button>
        <button type="button" onClick={() => onPointerDragEnd?.(identity)}>
          End {surface.surface_id}
        </button>
      </span>
    );
  },
}));

import { DockviewLayoutAdapter } from "./DockviewLayoutAdapter";

describe("DockviewLayoutAdapter pointer drag identity", () => {
  it("ignores stale cleanup from the previously active tab", async () => {
    const first = makeSurface("surface-1", { surface_type: "agents-overview" });
    const second = makeSurface("surface-2", { surface_type: "library" });
    render(<DockviewLayoutAdapter document={makeSingleGroupDocument([first, second])} />);
    const layout = document.querySelector<HTMLElement>(".wardian-workbench-layout");
    if (!layout) throw new Error("expected workbench layout");

    fireEvent.click(await screen.findByRole("button", { name: "Start surface-1" }));
    fireEvent.click(screen.getByRole("button", { name: "Start surface-2" }));
    await waitFor(() => {
      expect(layout).toHaveAttribute("data-pointer-drag-surface-id", "surface-2");
      expect(layout).toHaveAttribute("data-pointer-drag-pointer-id", "8");
    });

    fireEvent.click(screen.getByRole("button", { name: "End surface-1" }));
    expect(layout).toHaveAttribute("data-pointer-drag-surface-id", "surface-2");
    expect(layout).toHaveAttribute("data-pointer-drag-pointer-id", "8");

    fireEvent.click(screen.getByRole("button", { name: "End surface-2" }));
    await waitFor(() => {
      expect(layout).toHaveAttribute("data-pointer-drag-surface-id", "none");
      expect(layout).toHaveAttribute("data-pointer-drag-pointer-id", "none");
    });
  });
});
