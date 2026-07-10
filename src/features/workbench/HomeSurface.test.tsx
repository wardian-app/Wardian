import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { makeSurface } from "./workbenchTestUtils";
import { HomeSurface } from "./HomeSurface";

describe("HomeSurface", () => {
  it("renders derived empty-group discovery and recent reopen actions", () => {
    const onOpenSurface = vi.fn();
    const onReopenClosed = vi.fn();
    render(
      <HomeSurface
        group_id="group-empty"
        recently_closed={[{
          surface: makeSurface("closed-dashboard", { surface_type: "dashboard" }),
          previous_group_id: "group-empty",
          previous_index: 0,
        }]}
        on_open_surface={onOpenSurface}
        on_reopen_closed={onReopenClosed}
      />,
    );

    expect(screen.getByRole("heading", { name: "New Surface" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open Surface" }));
    expect(onOpenSurface).toHaveBeenCalledWith("group-empty");
    fireEvent.click(screen.getByRole("button", { name: "Reopen Dashboard" }));
    expect(onReopenClosed).toHaveBeenCalledOnce();
  });

  it("offers only the next reopenable surface when history contains older entries", () => {
    render(
      <HomeSurface
        group_id="group-empty"
        recently_closed={[
          {
            surface: makeSurface("closed-queue", { surface_type: "queue" }),
            previous_group_id: "group-empty",
            previous_index: 0,
          },
          {
            surface: makeSurface("closed-dashboard", { surface_type: "dashboard" }),
            previous_group_id: "group-empty",
            previous_index: 1,
          },
        ]}
        on_open_surface={() => {}}
        on_reopen_closed={() => {}}
      />,
    );

    expect(screen.getByRole("button", { name: "Reopen Queue" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reopen Dashboard" })).not.toBeInTheDocument();
  });
});
