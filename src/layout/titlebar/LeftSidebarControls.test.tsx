import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { LeftSidebarControls } from "./LeftSidebarControls";
import type { AppTelemetry } from "../../types";

describe("LeftSidebarControls telemetry", () => {
  it("includes Wardian app telemetry when no agents are present", () => {
    const appTelemetry: AppTelemetry = {
      cpu_usage: 2.5,
      memory_mb: 128.4,
    };

    render(
      <LeftSidebarControls
        leftCollapsed={false}
        setLeftCollapsed={vi.fn()}
        telemetry={{}}
        appTelemetry={appTelemetry}
        agents={[]}
        offAgentIds={new Set()}
        titlebarTelemetryVisible={true}
      />,
    );

    expect(screen.getByText("CPU 2.5%")).toBeInTheDocument();
    expect(screen.getByText("MEM 128MB")).toBeInTheDocument();
  });

  it("hides cpu memory and active telemetry when disabled", () => {
    const appTelemetry: AppTelemetry = {
      cpu_usage: 2.5,
      memory_mb: 128.4,
    };

    render(
      <LeftSidebarControls
        leftCollapsed={false}
        setLeftCollapsed={vi.fn()}
        telemetry={{}}
        appTelemetry={appTelemetry}
        agents={[]}
        offAgentIds={new Set()}
        titlebarTelemetryVisible={false}
      />,
    );

    expect(screen.queryByText("CPU 2.5%")).not.toBeInTheDocument();
    expect(screen.queryByText("MEM 128MB")).not.toBeInTheDocument();
    expect(screen.queryByText("0 Active")).not.toBeInTheDocument();
    expect(screen.getByTitle("Hide Left Sidebar")).toBeInTheDocument();
  });
});
