import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { QueueView } from "./QueueView";
import { useQueueStore } from "../store/useQueueStore";

vi.mocked(invoke).mockResolvedValue([]);

function resetStore() {
  useQueueStore.setState({ items: [], _agentBuffers: {}, _workflowLastOutput: {} });
}

describe("QueueView", () => {
  beforeEach(resetStore);

  it("shows empty state when no items", () => {
    render(<QueueView />);
    expect(screen.getByText("No completions yet.")).toBeInTheDocument();
  });

  it("renders an agent completion item", () => {
    useQueueStore.setState({
      items: [{
        id: "item-1",
        type: "agent_completed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Done writing tests.",
      }],
    });
    render(<QueueView />);
    expect(screen.getByText("My Coder")).toBeInTheDocument();
    expect(screen.getByText("Done writing tests.")).toBeInTheDocument();
    expect(screen.getByText("Completed")).toBeInTheDocument();
  });

  it("renders a failed workflow item with error text", () => {
    useQueueStore.setState({
      items: [{
        id: "item-2",
        type: "workflow_completed",
        timestamp: Date.now(),
        read: false,
        workflow_name: "CI Pipeline",
        status: "failed",
        error: "Timeout after 30s",
      }],
    });
    render(<QueueView />);
    expect(screen.getByText("CI Pipeline")).toBeInTheDocument();
    expect(screen.getByText("Failed")).toBeInTheDocument();
    expect(screen.getByText("Timeout after 30s")).toBeInTheDocument();
  });

  it("renders a completed workflow item with summary when present", () => {
    useQueueStore.setState({
      items: [{
        id: "item-3",
        type: "workflow_completed",
        timestamp: Date.now(),
        read: false,
        workflow_name: "Data Pipeline",
        status: "completed",
        summary: "Processed 42 records.",
      }],
    });
    render(<QueueView />);
    expect(screen.getByText("Processed 42 records.")).toBeInTheDocument();
  });

  it("dismiss button removes item", () => {
    useQueueStore.setState({
      items: [{
        id: "item-1",
        type: "agent_completed",
        timestamp: Date.now(),
        read: false,
        agent_name: "My Coder",
        summary: "Done.",
      }],
    });
    render(<QueueView />);
    fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    expect(screen.queryByText("My Coder")).not.toBeInTheDocument();
    expect(screen.getByText("No completions yet.")).toBeInTheDocument();
  });

  it("mark all read button appears and calls markAllRead", () => {
    useQueueStore.setState({
      items: [{
        id: "item-1",
        type: "agent_completed",
        timestamp: Date.now(),
        read: false,
        agent_name: "My Coder",
        summary: "Done.",
      }],
    });
    render(<QueueView />);
    fireEvent.click(screen.getByRole("button", { name: /mark all read/i }));
    expect(useQueueStore.getState().items[0].read).toBe(true);
  });
});
