import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CustomTitleBar } from "./CustomTitleBar";

const titlebarProps = {
  viewMode: "grid" as const,
  setViewMode: vi.fn(),
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
