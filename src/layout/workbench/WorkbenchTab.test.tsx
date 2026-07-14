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

  it("lets an adapter ignore stale cleanup from a previously active tab", () => {
    type DragIdentity = {
      surface_id: string;
      source_group_id: string;
      pointer_id: number;
    };
    let activeIdentity: DragIdentity | null = null;
    const onPointerDragStart = vi.fn((identity: DragIdentity) => {
      activeIdentity = identity;
    });
    const onPointerDragEnd = vi.fn((identity: DragIdentity) => {
      if (
        activeIdentity?.surface_id === identity.surface_id
        && activeIdentity.source_group_id === identity.source_group_id
        && activeIdentity.pointer_id === identity.pointer_id
      ) activeIdentity = null;
    });
    const first = makeSurface("surface-1", { surface_type: "agents-overview" });
    const second = makeSurface("surface-2", { surface_type: "library" });
    render(
      <>
        <div role="tab" aria-label="Agents Overview">
          <WorkbenchTab
            surface={first}
            title="Agents Overview"
            group_id="group-1"
            on_pointer_drag_start={onPointerDragStart}
            on_pointer_drag_end={onPointerDragEnd}
          />
        </div>
        <div role="tab" aria-label="Library">
          <WorkbenchTab
            surface={second}
            title="Library"
            group_id="group-1"
            on_pointer_drag_start={onPointerDragStart}
            on_pointer_drag_end={onPointerDragEnd}
          />
        </div>
      </>,
    );

    fireEvent.pointerDown(screen.getByRole("tab", { name: "Agents Overview" }), { pointerId: 7 });
    fireEvent.pointerDown(screen.getByRole("tab", { name: "Library" }), { pointerId: 8 });
    expect(activeIdentity).toEqual({
      surface_id: "surface-2",
      source_group_id: "group-1",
      pointer_id: 8,
    });

    fireEvent.pointerUp(window, { pointerId: 7 });
    expect(activeIdentity).toEqual({
      surface_id: "surface-2",
      source_group_id: "group-1",
      pointer_id: 8,
    });

    fireEvent.pointerUp(window, { pointerId: 8 });
    expect(activeIdentity).toBeNull();
  });
});
