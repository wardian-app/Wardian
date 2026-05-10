import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceTabs } from "./WorkspaceTabs";
import { useQueueStore } from "../../store/useQueueStore";

vi.mocked(invoke).mockResolvedValue([]);

function resetStore() {
  useQueueStore.setState({ items: [], _agentBuffers: {}, _workflowLastOutput: {} });
}

describe("WorkspaceTabs queue badge", () => {
  beforeEach(resetStore);

  it("does not show a badge when there are no unread items", () => {
    render(<WorkspaceTabs viewMode="grid" setViewMode={() => {}} />);
    expect(screen.queryByTestId("queue-unread-badge")).not.toBeInTheDocument();
  });

  it("shows a badge with unread count when there are unread items", () => {
    useQueueStore.setState({
      items: [
        { id: "1", type: "agent_completed", timestamp: Date.now(), read: false, agent_name: "A", summary: "s" },
        { id: "2", type: "agent_completed", timestamp: Date.now(), read: true, agent_name: "B", summary: "s" },
      ],
    });
    render(<WorkspaceTabs viewMode="grid" setViewMode={() => {}} />);
    expect(screen.getByTestId("queue-unread-badge")).toHaveTextContent("1");
  });

  it("shows 9+ when unread count exceeds 9", () => {
    useQueueStore.setState({
      items: Array.from({ length: 11 }, (_, i) => ({
        id: String(i),
        type: "agent_completed" as const,
        timestamp: Date.now(),
        read: false,
        agent_name: "A",
        summary: "s",
      })),
    });
    render(<WorkspaceTabs viewMode="grid" setViewMode={() => {}} />);
    expect(screen.getByTestId("queue-unread-badge")).toHaveTextContent("9+");
  });
});
