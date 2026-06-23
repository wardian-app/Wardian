import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { GardenContextMenu } from "./GardenContextMenu";

describe("GardenContextMenu", () => {
  it("always offers Reset layout and triggers it on click", async () => {
    const onResetLayout = vi.fn();
    const onClose = vi.fn();
    render(
      <GardenContextMenu
        x={10}
        y={10}
        agentId={null}
        onOpenAgent={vi.fn()}
        onResetLayout={onResetLayout}
        onClose={onClose}
      />,
    );
    expect(screen.queryByText("Open in Grid")).not.toBeInTheDocument();
    await userEvent.click(screen.getByTestId("garden-reset-layout"));
    expect(onResetLayout).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalled();
  });

  it("offers Open in Grid only when opened over an agent unit", async () => {
    const onOpenAgent = vi.fn();
    render(
      <GardenContextMenu
        x={10}
        y={10}
        agentId="a1"
        onOpenAgent={onOpenAgent}
        onResetLayout={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await userEvent.click(screen.getByText("Open in Grid"));
    expect(onOpenAgent).toHaveBeenCalledWith("a1");
  });
});
