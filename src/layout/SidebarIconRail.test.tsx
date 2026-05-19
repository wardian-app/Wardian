import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SidebarIconRail } from "./SidebarIconRail";

describe("SidebarIconRail density", () => {
  it("uses original-size icons inside roomy activity rail slots", () => {
    render(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        onToggleUserTerminal={vi.fn()}
      />,
    );

    expect(screen.getByTestId("sidebar-icon-rail")).toHaveClass("gap-3");
    const explorerIcon = screen.getByTestId("sidebar-tab-explorer").querySelector("svg");
    expect(screen.getByTestId("sidebar-tab-explorer")).toHaveClass("p-3");
    expect(explorerIcon).toHaveClass("w-6");
    expect(explorerIcon).toHaveClass("h-6");
  });

  it("exposes a non-blocking Getting Started help entry point from the app shell", () => {
    render(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        onToggleUserTerminal={vi.fn()}
      />,
    );

    expect(screen.getByTestId("sidebar-help-getting-started")).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/getting-started",
    );
  });
});
