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
    expect(document.body.textContent).toContain("My CoderAgent task completed");
    expect(screen.getByText("Agent task completed")).toBeInTheDocument();
    expect(screen.getByText("My Coder")).toBeInTheDocument();
    expect(screen.getByText("Done writing tests.")).toBeInTheDocument();
    expect(screen.queryByText("Completed")).not.toBeInTheDocument();
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
    expect(document.body.textContent).toContain("CI PipelineWorkflow failed");
    expect(screen.getByText("CI Pipeline")).toBeInTheDocument();
    expect(screen.getByText("Workflow failed")).toBeInTheDocument();
    expect(screen.queryByText("Failed")).not.toBeInTheDocument();
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
    expect(document.body.textContent).toContain("Data PipelineWorkflow completed");
    expect(screen.getByText("Workflow completed")).toBeInTheDocument();
    expect(screen.getByText("Processed 42 records.")).toBeInTheDocument();
  });

  it("collapses and expands long queue summaries", () => {
    useQueueStore.setState({
      items: [{
        id: "item-long",
        type: "agent_completed",
        timestamp: Date.now(),
        read: false,
        agent_name: "My Coder",
        summary: [
          "First line",
          "Second line",
          "Third line",
          "Fourth line",
          "Fifth line",
          "Sixth line",
        ].join("\n"),
      }],
    });
    render(<QueueView />);

    const summary = screen.getByTestId("queue-item-summary-item-long");
    expect(summary).toHaveClass("line-clamp-4");

    const toggle = screen.getByRole("button", { name: /show full summary/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(toggle);

    expect(summary).not.toHaveClass("line-clamp-4");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: /collapse summary/i })).toBeInTheDocument();
  });

  it("clear item button removes item", () => {
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
    fireEvent.click(screen.getByRole("button", { name: /clear item/i }));
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

  it("uses matching highlighted header button treatment for mark and clear actions", () => {
    useQueueStore.setState({
      items: [{
        id: "item-1",
        type: "agent_completed",
        timestamp: Date.now(),
        read: true,
        agent_name: "My Coder",
        summary: "Done.",
      }],
    });

    render(<QueueView />);

    expect(screen.getByRole("button", { name: /mark all read/i })).toHaveClass(
      "rounded-md",
      "px-2",
      "py-1",
      "hover:bg-wardian-card-bg-muted",
    );
    expect(screen.getByRole("button", { name: /clear read/i })).toHaveClass(
      "rounded-md",
      "px-2",
      "py-1",
      "hover:bg-wardian-card-bg-muted",
    );
  });

  it("clear read button removes read items and keeps unread items", () => {
    useQueueStore.setState({
      items: [
        {
          id: "read-item",
          type: "agent_completed",
          timestamp: Date.now(),
          read: true,
          agent_name: "Read Agent",
          summary: "Old result.",
        },
        {
          id: "unread-item",
          type: "workflow_completed",
          timestamp: Date.now(),
          read: false,
          workflow_name: "Unread Workflow",
          status: "completed",
          summary: "Fresh result.",
        },
      ],
    });

    render(<QueueView />);
    fireEvent.click(screen.getByRole("button", { name: /clear read/i }));

    expect(screen.queryByText("Read Agent")).not.toBeInTheDocument();
    expect(screen.getByText("Unread Workflow")).toBeInTheDocument();
  });
});
