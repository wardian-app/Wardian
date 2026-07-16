import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AgentContextMenu, type AgentContextMenuProps } from "./AgentContextMenu";

function createProps(overrides: Partial<AgentContextMenuProps> = {}): AgentContextMenuProps {
  return {
    x: 12,
    y: 24,
    agentId: "agent-1",
    offAgentIds: new Set(),
    watchlists: [],
    onInitiateRename: vi.fn(),
    onQuery: vi.fn(),
    onPause: vi.fn(),
    onRestart: vi.fn(),
    onClear: vi.fn(),
    onAddToList: vi.fn(),
    onRemoveFromList: vi.fn(),
    onDelete: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
}

describe("AgentContextMenu", () => {
  it("renders optional primary actions before management actions and closes after invoking them", async () => {
    const onOpen = vi.fn();
    const onOpenToSide = vi.fn();
    const onClose = vi.fn();

    render(
      <AgentContextMenu
        {...createProps({ onOpen, onOpenToSide, onClose })}
      />,
    );

    const menu = screen.getByTestId("agent-context-menu");
    expect(within(menu).getAllByRole("button").slice(0, 4).map((button) => button.textContent)).toEqual([
      "Open",
      "Open to Side",
      "Rename",
      "Query",
    ]);

    fireEvent.click(within(menu).getByRole("button", { name: "Open" }));

    expect(onOpen).toHaveBeenCalledWith("agent-1");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("omits primary actions when callbacks are absent", () => {
    render(<AgentContextMenu {...createProps()} />);

    const menu = screen.getByTestId("agent-context-menu");
    expect(within(menu).queryByRole("button", { name: "Open" })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("button", { name: "Open to Side" })).not.toBeInTheDocument();
    expect(within(menu).getAllByRole("button")[0]).toHaveTextContent("Rename");
  });
});
