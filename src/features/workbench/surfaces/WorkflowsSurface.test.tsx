import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { WorkflowsSurface } from "./WorkflowsSurface";

vi.mock("../../../views/WorkflowsView", () => ({
  WorkflowsView: () => <div data-testid="mock-workflows-view" />,
}));

describe("WorkflowsSurface", () => {
  it("hides the keep-alive workflows renderer when the workbench surface is hidden", () => {
    render(<WorkflowsSurface surface_id="workflows-1" theme="light" visibility="hidden" />);

    const surface = screen.getByTestId("workflows-surface");
    expect(surface).toHaveAttribute("aria-hidden", "true");
    expect(surface).toHaveAttribute("data-surface-visibility", "hidden");
    expect(surface).toHaveStyle({ display: "none" });
  });

  it("keeps the workflows renderer visible when the workbench surface is active", () => {
    render(<WorkflowsSurface surface_id="workflows-1" theme="light" visibility="visible" />);

    const surface = screen.getByTestId("workflows-surface");
    expect(surface).toHaveAttribute("aria-hidden", "false");
    expect(surface).toHaveAttribute("data-surface-visibility", "visible");
    expect(surface).not.toHaveStyle({ display: "none" });
    expect(screen.getByTestId("mock-workflows-view")).toBeInTheDocument();
  });
});
