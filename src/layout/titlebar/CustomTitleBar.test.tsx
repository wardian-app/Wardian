import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CustomTitleBar } from "./CustomTitleBar";

const titlebarProps = {
  leftCollapsed: false,
  setLeftCollapsed: vi.fn(),
  rightCollapsed: false,
  setRightCollapsed: vi.fn(),
  leftSidebarWidth: 240,
  rightSidebarWidth: 240,
  telemetry: {},
  appTelemetry: { cpu_usage: 0, memory_mb: 0 },
  agents: [],
  offAgentIds: new Set<string>(),
  titlebarTelemetryVisible: true,
};

describe("CustomTitleBar density", () => {
  it("aligns the left titlebar zone to the compact left rail and content pane", () => {
    const { container, rerender } = render(<CustomTitleBar {...titlebarProps} />);
    const titlebar = container.firstElementChild as HTMLElement;

    expect(titlebar.style.getPropertyValue("--titlebar-left-width")).toBe("288px");

    rerender(<CustomTitleBar {...titlebarProps} leftCollapsed />);

    expect(titlebar.style.getPropertyValue("--titlebar-left-width")).toBe("48px");
  });

  it("keeps titlebar zones aligned with resized sidebars", () => {
    const { container } = render(
      <CustomTitleBar
        {...titlebarProps}
        leftSidebarWidth={312}
        rightSidebarWidth={276}
      />,
    );
    const titlebar = container.firstElementChild as HTMLElement;

    expect(titlebar.style.getPropertyValue("--titlebar-left-width")).toBe("360px");
    expect(titlebar.style.getPropertyValue("--titlebar-right-width")).toBe("276px");
  });
});

describe("CustomTitleBar navigation", () => {
  it("keeps a quiet draggable center without fixed commands or global-view launchers", () => {
    render(<CustomTitleBar {...titlebarProps} />);

    const center = screen.getByTestId("titlebar-center");
    expect(center).toHaveAttribute("data-navigation-mode", "workbench");
    expect(center).toHaveAttribute("data-tauri-drag-region");
    expect(center).toBeEmptyDOMElement();
    expect(within(center).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Quick Open" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Commands" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Grid" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Dashboard" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Queue" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Graph" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Garden" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Library" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Workflows" })).not.toBeInTheDocument();
  });

  it("preserves telemetry, sidebar toggles, and window controls", () => {
    const setLeftCollapsed = vi.fn();
    const setRightCollapsed = vi.fn();
    const { container } = render(
      <CustomTitleBar
        {...titlebarProps}
        setLeftCollapsed={setLeftCollapsed}
        setRightCollapsed={setRightCollapsed}
      />,
    );

    expect(container.firstElementChild).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByText("CPU 0.0%")).toBeInTheDocument();
    expect(screen.getByText("MEM 0MB")).toBeInTheDocument();
    expect(screen.getByText("0 Active")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Hide Left Sidebar" }));
    fireEvent.click(screen.getByRole("button", { name: "Hide Agent Roster" }));
    expect(setLeftCollapsed).toHaveBeenCalledWith(true);
    expect(setRightCollapsed).toHaveBeenCalledWith(true);

    expect(screen.getByRole("button", { name: "Minimize" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Maximize" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close" })).toBeInTheDocument();
  });

  it("disables sidebar toggles but preserves window controls during reset", () => {
    render(
      <CustomTitleBar
        {...titlebarProps}
        workbenchBusy
      />,
    );

    expect(screen.getByRole("button", { name: "Hide Left Sidebar" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Hide Agent Roster" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Minimize" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Maximize" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Close" })).toBeEnabled();
  });
});
