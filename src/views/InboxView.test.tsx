import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { InboxView } from "./InboxView";
import { useQueueStore } from "../store/useQueueStore";
import { normalizeQueuePreferences } from "../features/queue/queueFilters";

vi.mocked(invoke).mockResolvedValue([]);

function resetStore() {
  useQueueStore.setState({
    items: [],
    inboxNotificationsTruncated: false,
    inboxNotificationsNextOffset: null,
    loadingMoreInboxNotifications: false,
    _agentBuffers: {},
    _automationLastOutput: {},
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

  it("shows when the notification projection is partial", () => {
    useQueueStore.setState({ inboxNotificationsTruncated: true });

    render(<InboxView />);

    expect(screen.getByRole("status")).toHaveTextContent("200 newest Inbox notifications");
  });

  it("offers one more bounded notification page", async () => {
    const loadMoreInboxNotifications = vi.fn(async () => undefined);
    useQueueStore.setState({
      inboxNotificationsTruncated: true,
      inboxNotificationsNextOffset: 200,
      loadMoreInboxNotifications,
    });

    render(<InboxView />);
    fireEvent.click(screen.getByRole("button", { name: /load next 200/i }));
    await waitFor(() => expect(loadMoreInboxNotifications).toHaveBeenCalledOnce());
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

    const { unmount } = render(<InboxView onOpenAgent={onOpenAgent} onSendAgentPrompt={onSendAgentPrompt} />);

    expect(screen.getByText("Action required")).toBeInTheDocument();
    expect(screen.getByTestId("queue-item-summary-item-action")).toHaveTextContent("Do you want to proceed? 1. Yes 2. No");
    fireEvent.click(screen.getByRole("button", { name: /open agent terminal/i }));
    expect(onOpenAgent).toHaveBeenCalledWith("sess-1");

    expect(screen.queryByLabelText("Quick response")).not.toBeInTheDocument();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Send action response 1: Yes" }));
    });
    expect(onSendAgentPrompt).toHaveBeenCalledWith("sess-1", "1", "item-action");
    expect(screen.getByRole("button", { name: "Send action response 1: Yes" })).toBeDisabled();
    expect(useQueueStore.getState().items[0]).toMatchObject({
      provider_choice_sent: "1",
      read: true,
    });

    unmount();
    render(<InboxView onSendAgentPrompt={onSendAgentPrompt} />);
    const choiceAfterRemount = screen.getByRole("button", { name: "Send action response 1: Yes" });
    expect(choiceAfterRemount).toBeDisabled();
    fireEvent.click(choiceAfterRemount);
    expect(onSendAgentPrompt).toHaveBeenCalledTimes(1);
  });

  it("does not acknowledge a desktop Inbox choice when delivery fails", async () => {
    const onSendAgentPrompt = vi.fn().mockRejectedValue(new Error("provider unavailable"));
    useQueueStore.setState({
      items: [{
        id: "item-action-failed",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Proceed?\n1. Yes",
      }],
    });

    render(<InboxView onSendAgentPrompt={onSendAgentPrompt} />);
    fireEvent.click(screen.getByRole("button", { name: "Send action response 1: Yes" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("provider unavailable");
    expect(useQueueStore.getState().items[0]).toMatchObject({ read: false });
    expect(useQueueStore.getState().items[0].provider_choice_sent).toBeUndefined();
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

  it("renders structured provider questions with read-only options and Open agent", () => {
    const onOpenAgent = vi.fn();
    useQueueStore.setState({
      items: [{
        id: "structured-question",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Which environment?",
        evidence_id: "provider-question:sess-1:codex:call-1",
        evidence_source: "provider_runtime",
        provider_question: {
          provider: "codex",
          call_id: "call-1",
          questions: [{
            header: "Target",
            prompt: "Which environment?",
            options: [
              { label: "Staging", description: "Use the test environment." },
              { label: "Production", description: "Use the live environment." },
            ],
          }],
        },
      }],
    });

    render(<InboxView onOpenAgent={onOpenAgent} onSendAgentPrompt={vi.fn()} />);

    expect(screen.getByText("Which environment?")).toBeVisible();
    expect(screen.getByText("Staging")).toBeVisible();
    expect(screen.getByTestId("provider-question-details")).toHaveTextContent("Use the test environment.");
    expect(screen.getByRole("button", { name: "Open agent terminal" })).toBeVisible();
    expect(screen.queryByRole("button", { name: /send action response/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Staging" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open agent terminal" }));
    expect(onOpenAgent).toHaveBeenCalledWith("sess-1");
  });

  it("does not replay a provider choice while delivery recovery is unresolved", () => {
    const onSendAgentPrompt = vi.fn(async () => undefined);
    useQueueStore.setState({
      items: [{
        id: "item-action-pending",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Proceed?\n1. Yes",
        provider_choice_pending: "1",
      }],
    });

    render(<InboxView onSendAgentPrompt={onSendAgentPrompt} />);

    const choice = screen.getByRole("button", { name: "Send action response 1: Yes" });
    expect(choice).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("Response delivery is uncertain");
    expect(screen.queryByRole("button", { name: /clear item/i })).not.toBeInTheDocument();
    fireEvent.click(choice);
    expect(onSendAgentPrompt).not.toHaveBeenCalled();
  });

  it("does not mark an uncertain provider choice read when opening the agent", () => {
    const onOpenAgent = vi.fn();
    useQueueStore.setState({
      items: [{
        id: "item-action-open-pending",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Proceed?\n1. Yes",
        provider_choice_pending: "1",
      }],
    });

    render(<InboxView onOpenAgent={onOpenAgent} />);

    fireEvent.click(screen.getByRole("button", { name: "Open agent terminal" }));

    expect(onOpenAgent).toHaveBeenCalledWith("sess-1");
    expect(useQueueStore.getState().items[0].read).toBe(false);
  });

  it("recovers a sent provider choice acknowledgement without replaying it", () => {
    const onSendAgentPrompt = vi.fn();
    useQueueStore.setState({
      items: [{
        id: "item-action-sent-unread",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Proceed?\n1. Yes",
        provider_choice_sent: "1",
      }],
    });

    render(<InboxView onSendAgentPrompt={onSendAgentPrompt} />);

    expect(screen.getByText("Response sent. Inbox status may need updating.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send action response 1: Yes" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Retry Inbox status" }));

    expect(useQueueStore.getState().items[0].read).toBe(true);
    expect(onSendAgentPrompt).not.toHaveBeenCalled();
  });

  it("does not replay a provider choice already acknowledged by the shared Inbox", () => {
    const onSendAgentPrompt = vi.fn(async () => undefined);
    useQueueStore.setState({
      items: [{
        id: "item-action-sent",
        type: "action_needed",
        timestamp: Date.now(),
        read: true,
        agent_session_id: "sess-1",
        agent_name: "My Coder",
        summary: "Proceed?\n1. Yes",
        provider_choice_sent: "1",
      }],
    });

    render(<InboxView onSendAgentPrompt={onSendAgentPrompt} />);

    const choice = screen.getByRole("button", { name: "Send action response 1: Yes" });
    expect(choice).toBeDisabled();
    fireEvent.click(choice);
    expect(onSendAgentPrompt).not.toHaveBeenCalled();
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
    expect(screen.queryByLabelText("Desktop alert for automation failures")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Sound alert for action required")).not.toBeInTheDocument();

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

  it("renders a failed automation item with error text", () => {
    useQueueStore.setState({
      items: [{
        id: "item-2",
        type: "automation_completed",
        timestamp: Date.now(),
        read: false,
        automation_name: "CI Pipeline",
        status: "failed",
        error: "Timeout after 30s",
      }],
    });
    render(<InboxView />);
    expect(document.body.textContent).toContain("CI PipelineAutomation failed");
    expect(screen.getByText("CI Pipeline")).toBeInTheDocument();
    expect(screen.getByText("Automation failed")).toBeInTheDocument();
    expect(screen.queryByText("Failed")).not.toBeInTheDocument();
    expect(screen.getByText("Timeout after 30s")).toBeInTheDocument();
  });

  it("renders a completed automation item with summary when present", () => {
    useQueueStore.setState({
      items: [{
        id: "item-3",
        type: "automation_completed",
        timestamp: Date.now(),
        read: false,
        automation_name: "Data Pipeline",
        status: "completed",
        summary: "Processed 42 records.",
      }],
    });
    render(<InboxView />);
    expect(document.body.textContent).toContain("Data PipelineAutomation completed");
    expect(screen.getByText("Automation completed")).toBeInTheDocument();
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

  it("does not offer local acknowledgement controls for automation approval projections", () => {
    useQueueStore.setState({
      items: [{
        id: "automation-approval:wf:run:gate",
        type: "approval_request",
        timestamp: Date.now(),
        read: false,
        notification_title: "Release gate",
        summary: "Approve the deployment?",
        approval_choices: ["Approve", "Reject"],
        automation_approval: {
          blueprint_id: "wf",
          blueprint_path: "automation.json",
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

  it("resolves automation approval projections from the Inbox", async () => {
    useQueueStore.setState({
      items: [{
        id: "automation-approval:wf:run:gate",
        type: "approval_request",
        timestamp: Date.now(),
        read: false,
        notification_title: "Release gate",
        summary: "Approve the deployment?",
        approval_choices: ["Approve", "Reject"],
        automation_approval: {
          blueprint_id: "wf",
          blueprint_path: "automation.json",
          run_id: "run",
          node: "gate",
        },
      }],
    });

    render(<InboxView />);
    fireEvent.click(screen.getByRole("button", { name: /^approve$/i }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("automation_approve", {
        blueprintId: "wf",
        runId: "run",
        blueprintPath: "automation.json",
        node: "gate",
        granted: true,
        actor: "user",
        note: null,
      });
    });
  });

  it("keeps an automation approval actionable and reports a resolution failure", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("run is no longer awaiting approval"));
    useQueueStore.setState({
      items: [{
        id: "automation-approval:wf:run:gate",
        type: "approval_request",
        timestamp: Date.now(),
        read: false,
        notification_title: "Release gate",
        summary: "Approve the deployment?",
        approval_choices: ["Approve", "Reject"],
        automation_approval: {
          blueprint_id: "wf",
          blueprint_path: "automation.json",
          run_id: "run",
          node: "gate",
        },
      }],
    });

    render(<InboxView />);
    fireEvent.click(screen.getByRole("button", { name: /^reject$/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not resolve this approval: run is no longer awaiting approval",
    );
    expect(screen.getByRole("button", { name: /^approve$/i })).toBeEnabled();
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
          type: "automation_completed",
          timestamp: Date.now(),
          read: false,
          automation_name: "Unread Automation",
          status: "completed",
          summary: "Fresh result.",
        },
      ],
    });

    render(<InboxView />);
    fireEvent.click(screen.getByRole("button", { name: /clear read/i }));

    expect(screen.queryByText("Read Agent")).not.toBeInTheDocument();
    expect(screen.getByText("Unread Automation")).toBeInTheDocument();
  });

  it("keeps read action-needed prompts when clearing read completions", () => {
    useQueueStore.setState({
      items: [{
        id: "read-action",
        type: "action_needed",
        timestamp: Date.now(),
        read: true,
        agent_name: "Read Action",
        summary: "Choose an action.",
      }],
    });

    render(<InboxView />);

    expect(screen.getByRole("button", { name: /clear read/i })).toBeDisabled();
    expect(screen.getByText("Read Action")).toBeInTheDocument();
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

  it("loads older Inbox items only after the list is scrolled to its end", () => {
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

    expect(screen.getByText("Agent 79")).toBeInTheDocument();
    expect(screen.queryByText("Agent 80")).not.toBeInTheDocument();

    const scrollRegion = screen.getByTestId("inbox-scroll-region");
    Object.defineProperties(scrollRegion, {
      clientHeight: { configurable: true, value: 100 },
      scrollHeight: { configurable: true, value: 1000 },
      scrollTop: { configurable: true, value: 0, writable: true },
    });
    fireEvent.scroll(scrollRegion);
    expect(screen.queryByText("Agent 80")).not.toBeInTheDocument();

    scrollRegion.scrollTop = 900;
    fireEvent.scroll(scrollRegion);
    expect(screen.getByText("Agent 119")).toBeInTheDocument();
  });
});
