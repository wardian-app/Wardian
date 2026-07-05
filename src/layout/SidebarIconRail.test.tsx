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

  it("shows a source control pending-change badge only when changes exist", () => {
    const { rerender } = render(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        settingsOpen={false}
        sourceControlChangeCount={12}
        onToggleUserTerminal={vi.fn()}
        onToggleSettings={vi.fn()}
      />,
    );

    expect(screen.getByTestId("sidebar-tab-git-badge")).toHaveTextContent("12");
    expect(screen.getByTestId("sidebar-tab-git-badge")).toHaveAttribute("aria-label", "12 pending source control changes");

    rerender(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        settingsOpen={false}
        sourceControlChangeCount={0}
        onToggleUserTerminal={vi.fn()}
        onToggleSettings={vi.fn()}
      />,
    );

    expect(screen.queryByTestId("sidebar-tab-git-badge")).not.toBeInTheDocument();
  });

  it("shows a source control progress marker while git is refreshing", () => {
    render(
      <SidebarIconRail
        activeTab="explorer"
        setActiveTab={vi.fn()}
        setCollapsed={vi.fn()}
        userTerminalOpen={false}
        settingsOpen={false}
        sourceControlBusy={true}
        onToggleUserTerminal={vi.fn()}
        onToggleSettings={vi.fn()}
      />,
    );

    expect(screen.getByTestId("sidebar-tab-git-progress")).toHaveAttribute(
      "aria-label",
      "Source control is refreshing",
    );
  });
});
