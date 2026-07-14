import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { makeSurface } from "../../features/workbench/workbenchTestUtils";
import { WorkbenchTab } from "./WorkbenchTab";

describe("WorkbenchTab", () => {
  it("ends a scoped pointer drag only for its originating pointer id", () => {
    const onPointerDragStart = vi.fn();
    const onPointerDragEnd = vi.fn();
    const surface = makeSurface("surface-1", { surface_type: "agents-overview" });
    const view = render(
      <div role="tab" aria-label="Agents Overview">
        <WorkbenchTab
          surface={surface}
          title="Agents Overview"
          group_id="group-1"
          on_pointer_drag_start={onPointerDragStart}
          on_pointer_drag_end={onPointerDragEnd}
        />
      </div>,
    );
    const tab = screen.getByRole("tab", { name: "Agents Overview" });
    expect(tab.querySelector('[data-surface-icon="agents-overview"]')).toBeInTheDocument();

    fireEvent.pointerDown(tab, { pointerId: 7 });
    expect(onPointerDragStart).toHaveBeenCalledWith({
      surface_id: "surface-1",
      source_group_id: "group-1",
      pointer_id: 7,
    });
    fireEvent.pointerUp(window, { pointerId: 8 });
    fireEvent.pointerCancel(window, { pointerId: 8 });
    expect(onPointerDragEnd).not.toHaveBeenCalled();

    fireEvent.pointerUp(window, { pointerId: 7 });
    expect(onPointerDragEnd).toHaveBeenCalledOnce();
    expect(onPointerDragEnd).toHaveBeenLastCalledWith({
      surface_id: "surface-1",
      source_group_id: "group-1",
      pointer_id: 7,
    });

    fireEvent.pointerDown(tab, { pointerId: 9 });
    fireEvent.pointerDown(tab, { pointerId: 10 });
    expect(onPointerDragEnd).toHaveBeenCalledTimes(2);
    expect(onPointerDragEnd).toHaveBeenLastCalledWith({
      surface_id: "surface-1",
      source_group_id: "group-1",
      pointer_id: 9,
    });
    fireEvent.pointerCancel(window, { pointerId: 9 });
    expect(onPointerDragEnd).toHaveBeenCalledTimes(2);
    fireEvent.pointerCancel(window, { pointerId: 10 });
    expect(onPointerDragEnd).toHaveBeenCalledTimes(3);

    fireEvent.pointerDown(tab, { pointerId: 11 });
    view.unmount();
    expect(onPointerDragEnd).toHaveBeenCalledTimes(4);
  });

});
