import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { InboxView } from "./InboxView";
import { useQueueStore } from "../store/useQueueStore";
import { normalizeQueuePreferences } from "../features/queue/queueFilters";

vi.mocked(invoke).mockResolvedValue([]);

function resetStore() {
  useQueueStore.setState({
    items: [],
    _agentBuffers: {},
    _workflowLastOutput: {},
    _readNotificationIds: [],
    preferences: normalizeQueuePreferences({}),
  });
}

describe("InboxView", () => {
  beforeEach(resetStore);

  it("shows empty state when no items", () => {
    render(<InboxView />);
    expect(screen.getByText("No completions yet.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /first-run guide/i })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/getting-started",
    );
    expect(screen.getByRole("link", { name: /inbox guide/i })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/inbox",
    );
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
    render(<InboxView />);
    expect(document.body.textContent).toContain("My CoderAgent task completed");
    expect(screen.getByText("Agent task completed")).toBeInTheDocument();
    expect(screen.getByText("My Coder")).toBeInTheDocument();
    expect(screen.getByText("Done writing tests.")).toBeInTheDocument();
    expect(screen.queryByText("Completed")).not.toBeInTheDocument();
  });

  it("renders an action-needed item with inferred action choices", async () => {
    const onOpenAgent = vi.fn();
    const onSendAgentPrompt = vi.fn(async () => undefined);
    useQueueStore.setState({
      items: [{
        id: "item-action",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Do you want to proceed?\n1. Yes\n2. No",
      }],
    });

    render(<InboxView onOpenAgent={onOpenAgent} onSendAgentPrompt={onSendAgentPrompt} />);

    expect(screen.getByText("Action needed")).toBeInTheDocument();
    expect(screen.getByTestId("queue-item-summary-item-action")).toHaveTextContent("Do you want to proceed? 1. Yes 2. No");
    fireEvent.click(screen.getByRole("button", { name: /open agent terminal/i }));
    expect(onOpenAgent).toHaveBeenCalledWith("sess-1");

    expect(screen.queryByLabelText("Quick response")).not.toBeInTheDocument();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Send action response 1: Yes" }));
    });
    expect(onSendAgentPrompt).toHaveBeenCalledWith("sess-1", "1");
  });

  it("does not render action buttons when the provider did not expose explicit choices", () => {
    const onSendAgentPrompt = vi.fn(async () => undefined);
    useQueueStore.setState({
      items: [{
        id: "item-action-generic",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Approve file write?",
      }],
    });

    render(<InboxView onSendAgentPrompt={onSendAgentPrompt} />);

    expect(screen.getByText("Approve file write?")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /send action response/i })).not.toBeInTheDocument();
  });

  it("filters visible queue items by event type", () => {
    useQueueStore.setState((state) => ({
      preferences: {
        ...state.preferences,
        visible_event_types: {
          ...state.preferences.visible_event_types,
          agent_completed: false,
        },
      },
      items: [
        {
          id: "hidden-agent",
          type: "agent_completed",
          timestamp: Date.now(),
          read: false,
          agent_name: "Hidden Agent",
          summary: "Done.",
        },
        {
          id: "visible-action",
          type: "action_needed",
          timestamp: Date.now(),
          read: false,
          agent_name: "Visible Agent",
          summary: "Needs approval.",
        },
      ],
    }));

    render(<InboxView />);

    expect(screen.queryByText("Hidden Agent")).not.toBeInTheDocument();
    expect(screen.getByText("Visible Agent")).toBeInTheDocument();
  });

  it("filters from a compact header dropdown without showing alert rules", () => {
    useQueueStore.setState({
      items: [
        {
          id: "hidden-agent",
          type: "agent_completed",
          timestamp: Date.now(),
          read: false,
          agent_name: "Hidden Agent",
          summary: "Done.",
        },
        {
          id: "visible-action",
          type: "action_needed",
          timestamp: Date.now(),
          read: false,
          agent_name: "Visible Agent",
          summary: "Needs approval.",
        },
      ],
    });

    render(<InboxView />);

    expect(screen.getByRole("button", { name: /filter inbox events/i })).toHaveTextContent("Filter: All events");
    expect(screen.queryByLabelText("Desktop alert for workflow failures")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Sound alert for action needed")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /filter inbox events/i }));
    fireEvent.click(screen.getByLabelText("Show agent completions"));

    const { preferences } = useQueueStore.getState();
    expect(preferences.visible_event_types.agent_completed).toBe(false);
    expect(screen.queryByText("Hidden Agent")).not.toBeInTheDocument();
    expect(screen.getByText("Visible Agent")).toBeInTheDocument();
  });

  it("places the unread indicator near the card top-left", () => {
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

    render(<InboxView />);

    expect(screen.getByTestId("queue-unread-dot")).toHaveClass("left-2", "top-2");
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
    render(<InboxView />);
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
    render(<InboxView />);
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
    render(<InboxView />);

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
    render(<InboxView />);
    fireEvent.click(screen.getByRole("button", { name: /clear item/i }));
    expect(screen.queryByText("My Coder")).not.toBeInTheDocument();
    expect(screen.getByText("No completions yet.")).toBeInTheDocument();
  });

  it("does not offer local acknowledgement controls for workflow approval projections", () => {
    useQueueStore.setState({
      items: [{
        id: "workflow-approval:wf:run:gate",
        type: "approval_request",
        timestamp: Date.now(),
        read: false,
        notification_title: "Release gate",
        summary: "Approve the deployment?",
        approval_choices: ["Approve", "Reject"],
        workflow_approval: {
          blueprint_id: "wf",
          blueprint_path: "workflow.json",
          run_id: "run",
          node: "gate",
        },
      }],
    });

    render(<InboxView />);

    expect(screen.queryByRole("button", { name: /clear item/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Release gate"));
    expect(useQueueStore.getState().items[0].read).toBe(false);
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
    render(<InboxView />);
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

    render(<InboxView />);

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

    render(<InboxView />);
    fireEvent.click(screen.getByRole("button", { name: /clear read/i }));

    expect(screen.queryByText("Read Agent")).not.toBeInTheDocument();
    expect(screen.getByText("Unread Workflow")).toBeInTheDocument();
  });

  it("keeps queue cards from shrinking when the list overflows", () => {
    useQueueStore.setState({
      items: Array.from({ length: 24 }, (_, index) => ({
        id: `item-${index}`,
        type: "agent_completed",
        timestamp: Date.now() - index,
        read: false,
        agent_name: `Agent ${index}`,
        summary: `Completed task ${index}.`,
      })),
    });

    render(<InboxView />);

    const firstCard = screen.getByText("Agent 0").closest(".group");
    expect(firstCard).toHaveClass("shrink-0");
    expect(firstCard?.parentElement).toHaveClass("flex-1", "min-h-0", "overflow-y-auto");
  });

  it("bounds the initial backlog render and fills the rest after idle work", async () => {
    vi.useFakeTimers();
    try {
      useQueueStore.setState({
        items: Array.from({ length: 120 }, (_, index) => ({
          id: `item-${index}`,
          type: "agent_completed",
          timestamp: Date.now() - index,
          read: false,
          agent_name: `Agent ${index}`,
          summary: `Completed task ${index}.`,
        })),
      });

      render(<InboxView />);

      expect(screen.getByText("Agent 0")).toBeInTheDocument();
      expect(screen.getByText("Agent 79")).toBeInTheDocument();
      expect(screen.queryByText("Agent 80")).not.toBeInTheDocument();
      expect(screen.queryByText("Agent 119")).not.toBeInTheDocument();

      await act(async () => {
        vi.runOnlyPendingTimers();
      });

      expect(screen.getByText("Agent 119")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
