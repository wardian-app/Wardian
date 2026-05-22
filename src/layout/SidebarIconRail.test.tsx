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
        settingsOpen={false}
        onToggleUserTerminal={vi.fn()}
        onToggleSettings={vi.fn()}
      />,
    );

    expect(screen.getByTestId("sidebar-icon-rail")).toHaveClass("gap-3");
    const explorerIcon = screen.getByTestId("sidebar-tab-explorer").querySelector("svg");
    expect(screen.getByTestId("sidebar-tab-explorer")).toHaveClass("p-3");
    expect(explorerIcon).toHaveClass("w-6");
    expect(explorerIcon).toHaveClass("h-6");
  });

  it("does not reserve a persistent help slot on the icon rail", () => {
    render(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        settingsOpen={false}
        onToggleUserTerminal={vi.fn()}
        onToggleSettings={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("sidebar-help-getting-started")).not.toBeInTheDocument();
  });

  it("does not expose remote connections on the icon rail", () => {
    render(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        settingsOpen={false}
        onToggleUserTerminal={vi.fn()}
        onToggleSettings={vi.fn()}
      />,
    );

    expect(screen.queryByTitle("Remote Connections")).not.toBeInTheDocument();
  });
});
