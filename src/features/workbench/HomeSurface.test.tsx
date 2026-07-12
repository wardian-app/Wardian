import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { makeSurface } from "./workbenchTestUtils";
import { HomeSurface } from "./HomeSurface";
import { createCoreWorkbenchSurfaceRegistry } from "./coreSurfaceRegistry";

const registry = createCoreWorkbenchSurfaceRegistry();

describe("HomeSurface", () => {
  it("renders derived empty-group discovery and recent reopen actions", () => {
    const onOpenSurface = vi.fn();
    const onSelectSurface = vi.fn();
    const onReopenClosed = vi.fn();
    render(
      <HomeSurface
        group_id="group-empty"
        registry={registry}
        recently_closed={[{
          surface: makeSurface("closed-dashboard", { surface_type: "dashboard" }),
          previous_group_id: "group-empty",
          previous_index: 0,
        }]}
        on_open_surface={onOpenSurface}
        on_select_surface={onSelectSurface}
        on_reopen_closed={onReopenClosed}
      />,
    );

    expect(screen.getByRole("heading", { name: "Choose a surface" })).toBeInTheDocument();
    expect(screen.getByLabelText("Available surfaces").querySelectorAll("button")).toHaveLength(7);
    expect(screen.queryByRole("button", { name: /^File Editor/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Agents: Monitor active agents." }));
    expect(onSelectSurface).toHaveBeenCalledWith("agents-overview", "group-empty");
    fireEvent.click(screen.getByRole("button", { name: "Browse all surfaces" }));
    expect(onOpenSurface).toHaveBeenCalledWith("group-empty");
    fireEvent.click(screen.getByRole("button", { name: "Reopen Dashboard" }));
    expect(onReopenClosed).toHaveBeenCalledOnce();
  });

  it("offers only the next reopenable surface when history contains older entries", () => {
    render(
      <HomeSurface
        group_id="group-empty"
        registry={registry}
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
        on_select_surface={() => {}}
        on_reopen_closed={() => {}}
      />,
    );

    expect(screen.getByRole("button", { name: "Reopen Queue" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reopen Dashboard" })).not.toBeInTheDocument();
  });

  it("includes registered extension surfaces with safe fallback metadata", () => {
    const extensibleRegistry = createCoreWorkbenchSurfaceRegistry();
    const dashboard = extensibleRegistry.require("dashboard");
    extensibleRegistry.register({
      ...dashboard,
      type: "extension-tool",
      icon: "extension-tool",
      title: () => "Extension Tool",
    });
    const onSelectSurface = vi.fn();

    render(
      <HomeSurface
        group_id="group-empty"
        registry={extensibleRegistry}
        on_open_surface={() => {}}
        on_select_surface={onSelectSurface}
      />,
    );

    fireEvent.click(screen.getByRole("button", {
      name: "Extension Tool: Open the Extension Tool surface.",
    }));
    expect(onSelectSurface).toHaveBeenCalledWith("extension-tool", "group-empty");
  });
});
