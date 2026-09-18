import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { QueueItem } from "../../types";
import { RemoteInboxView } from "./RemoteInboxView";
import { useRemoteStore } from "./useRemoteStore";

const originalOpenAgent = useRemoteStore.getState().openAgent;
const originalRunInboxAction = useRemoteStore.getState().runInboxAction;
const originalSendPromptToAgent = useRemoteStore.getState().sendPromptToAgent;
const originalRefreshInbox = useRemoteStore.getState().refreshInbox;

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  useRemoteStore.setState({
    remoteQueueItems: [],
    providerChoiceRecoveryByItem: {},
    openAgent: originalOpenAgent,
    runInboxAction: originalRunInboxAction,
    sendPromptToAgent: originalSendPromptToAgent,
    refreshInbox: originalRefreshInbox,
  });
});

describe("RemoteInboxView", () => {
  it("loads older Inbox items only after the mobile list is scrolled to its end", () => {
    useRemoteStore.setState({
      remoteQueueItems: Array.from({ length: 120 }, (_, index) => ({
        id: `remote-item-${index}`,
        type: "agent_completed" as const,
        timestamp: Date.now() - index,
        read: false,
        agent_name: `Agent ${index}`,
        summary: `Completed task ${index}.`,
      })),
    });

    render(<RemoteInboxView />);

    expect(screen.getByText("Agent 79")).toBeInTheDocument();
    expect(screen.queryByText("Agent 80")).not.toBeInTheDocument();

    const scrollRegion = screen.getByTestId("remote-inbox-scroll-region");
    Object.defineProperties(scrollRegion, {
      clientHeight: { configurable: true, value: 100 },
      scrollHeight: { configurable: true, value: 1000 },
      scrollTop: { configurable: true, value: 900, writable: true },
    });
    fireEvent.scroll(scrollRegion);

    expect(screen.getByText("Agent 119")).toBeInTheDocument();
  });

  it("collapses long summaries and opens the related agent", () => {
    const openAgent = vi.fn().mockResolvedValue(undefined);
    const item: QueueItem = {
      id: "remote-inbox-1",
      type: "agent_update",
      timestamp: Date.now(),
      read: false,
      agent_session_id: "agent-1",
      notification_title: "Agent task completed",
      summary: "A long update that contains enough detail to need a collapsed preview.\n\n"
        + "The mobile card should start in the same compact state as the desktop Inbox card, "
        + "then reveal the complete message when the user asks for more details.",
    };
    useRemoteStore.setState({ remoteQueueItems: [item], openAgent });

    render(<RemoteInboxView />);

    const summary = screen.getByTestId("remote-queue-item-summary-remote-inbox-1");
    const toggle = screen.getByRole("button", { name: "Show full summary" });
    expect(screen.getByText("Agent task completed")).toBeVisible();
    expect(screen.getByTestId("remote-queue-unread-dot")).toBeVisible();
    expect(summary).toHaveClass("line-clamp-4");
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(toggle);

    expect(summary).toHaveClass("max-h-80");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Collapse summary" })).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Open agent terminal" }));
    expect(openAgent).toHaveBeenCalledWith("agent-1");
  });

  it("filters events and exposes durable triage controls", async () => {
    const runInboxAction = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      runInboxAction,
      remoteQueueItems: [
        {
          id: "unread-action",
          type: "action_needed",
          timestamp: Date.now(),
          read: false,
          agent_name: "Coder",
          summary: "Choose an action",
        },
        {
          id: "read-update",
          type: "agent_completed",
          timestamp: Date.now(),
          read: true,
          agent_name: "Reviewer",
          summary: "Reviewed the change",
        },
      ],
    });

    render(<RemoteInboxView />);

    expect(screen.getByText("Coder")).toBeVisible();
    expect(screen.getByText("Reviewer")).toBeVisible();
    fireEvent.change(screen.getByRole("combobox", { name: "Filter Inbox events" }), {
      target: { value: "action_needed" },
    });
    expect(screen.getByText("Coder")).toBeVisible();
    expect(screen.queryByText("Reviewer")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Mark all Inbox items read" }));
    await waitFor(() => expect(runInboxAction).toHaveBeenCalledWith("mark_all_read"));
    fireEvent.click(screen.getByRole("button", { name: "Clear read Inbox items" }));
    await waitFor(() => expect(runInboxAction).toHaveBeenCalledWith("clear_read"));
    expect(runInboxAction).toHaveBeenCalledWith("mark_all_read");
  });

  it("answers provider choices and resolves approval choices", async () => {
    const runInboxAction = vi.fn().mockResolvedValue(undefined);
    const sendPromptToAgent = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      runInboxAction,
      sendPromptToAgent,
      remoteQueueItems: [
        {
          id: "action-1",
          type: "action_needed",
          timestamp: Date.now(),
          read: false,
          agent_session_id: "agent-1",
          agent_name: "Coder",
          summary: "Proceed?\n1. Yes\n2. No",
        },
        {
          id: "approval-1",
          type: "approval_request",
          timestamp: Date.now(),
          read: false,
          notification_title: "Deploy",
          summary: "Approve deployment?",
          notification_status: "awaiting_reply",
          approval_choices: ["Approve", "Reject"],
          inbox_notification_id: "notification-1",
        },
      ],
    });

    render(<RemoteInboxView />);

    await fireEvent.click(screen.getByRole("button", { name: "Send action response 1: Yes" }));
    await fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(sendPromptToAgent).toHaveBeenCalledWith("agent-1", "1", "action-1");
    expect(runInboxAction).toHaveBeenCalledWith("mark_read", "action-1");
    expect(runInboxAction).toHaveBeenCalledWith("resolve_approval", "approval-1", "Approve");
  });

  it("renders structured provider questions without numeric response buttons", () => {
    const openAgent = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      openAgent,
      remoteQueueItems: [{
        id: "remote-structured-question",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "agent-1",
        agent_name: "Coder",
        summary: "Which files should be included?",
        evidence_id: "provider-question:agent-1:claude:claude-call-1",
        evidence_source: "provider_runtime",
        provider_question: {
          provider: "claude",
          call_id: "claude-call-1",
          questions: [{
            header: "Scope",
            prompt: "Which files should be included?",
            options: [{ label: "Frontend", description: "Use the frontend source tree." }],
          }],
        },
      }],
    });

    render(<RemoteInboxView />);

    expect(screen.getByText("Which files should be included?")).toBeVisible();
    expect(screen.getByText("Frontend")).toBeVisible();
    expect(screen.getByTestId("provider-question-details")).toHaveTextContent("Use the frontend source tree.");
    expect(screen.getByRole("button", { name: "Open agent terminal" })).toBeVisible();
    expect(screen.queryByRole("button", { name: /send action response/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Frontend" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open agent terminal" }));
    expect(openAgent).toHaveBeenCalledWith("agent-1");
  });

  it("does not resend a provider choice when Inbox acknowledgement fails", async () => {
    const runInboxAction = vi.fn()
      .mockRejectedValueOnce(new Error("Remote request failed: 503"))
      .mockResolvedValue(undefined);
    const sendPromptToAgent = vi.fn().mockResolvedValue(undefined);
    const refreshInbox = vi.fn().mockResolvedValue(true);
    useRemoteStore.setState({
      runInboxAction,
      refreshInbox,
      sendPromptToAgent,
      remoteQueueItems: [{
        id: "action-1",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "agent-1",
        agent_name: "Coder",
        summary: "Proceed?\n1. Yes",
      }],
    });

    render(<RemoteInboxView />);

    const choice = screen.getByRole("button", { name: "Send action response 1: Yes" });
    fireEvent.click(choice);
    await waitFor(() => expect(screen.getByRole("button", { name: "Retry Inbox status" })).toBeVisible());
    expect(sendPromptToAgent).toHaveBeenCalledWith("agent-1", "1", "action-1");
    expect(sendPromptToAgent).toHaveBeenCalledTimes(1);
    expect(runInboxAction).toHaveBeenCalledWith("mark_read", "action-1");
    expect(choice).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Retry Inbox status" }));
    await waitFor(() => expect(runInboxAction).toHaveBeenCalledTimes(2));
    expect(sendPromptToAgent).toHaveBeenCalledTimes(1);
  });

  it("keeps a durably sent choice disabled after acknowledgement failure and remount", async () => {
    const runInboxAction = vi.fn().mockRejectedValue(new Error("Remote request failed: 503"));
    const sendPromptToAgent = vi.fn().mockResolvedValue(undefined);
    let resolveRefresh!: (value: boolean) => void;
    const refreshPromise = new Promise<boolean>((resolve) => { resolveRefresh = resolve; });
    const refreshInbox = vi.fn().mockReturnValue(refreshPromise);
    useRemoteStore.setState({
      runInboxAction,
      refreshInbox,
      sendPromptToAgent,
      remoteQueueItems: [{
        id: "action-1",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "agent-1",
        agent_name: "Coder",
        summary: "Proceed?\n1. Yes",
      }],
    });

    render(<RemoteInboxView />);
    fireEvent.click(screen.getByRole("button", { name: "Send action response 1: Yes" }));
    await waitFor(() => expect(refreshInbox).toHaveBeenCalledTimes(1));
    expect(useRemoteStore.getState().providerChoiceRecoveryByItem["action-1"]).toBe("1");

    cleanup();
    render(<RemoteInboxView />);
    const choice = screen.getByRole("button", { name: "Send action response 1: Yes" });
    expect(choice).toBeDisabled();
    fireEvent.click(choice);
    expect(sendPromptToAgent).toHaveBeenCalledTimes(1);
    resolveRefresh(true);
  });

  it("keeps the provider choice disabled when uncertain delivery cannot be refreshed", async () => {
    const sendPromptToAgent = vi.fn().mockRejectedValue(new Error("Remote request failed: timeout"));
    const refreshInbox = vi.fn().mockResolvedValue(false);
    useRemoteStore.setState({
      refreshInbox,
      sendPromptToAgent,
      remoteQueueItems: [{
        id: "action-1",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "agent-1",
        agent_name: "Coder",
        summary: "Proceed?\n1. Yes",
      }],
    });

    render(<RemoteInboxView />);

    const choice = screen.getByRole("button", { name: "Send action response 1: Yes" });
    fireEvent.click(choice);
    await waitFor(() => expect(refreshInbox).toHaveBeenCalledTimes(1));

    expect(choice).toBeDisabled();
    expect(screen.getByText("Response delivery is uncertain. Check the agent before retrying.")).toBeVisible();
  });

  it("keeps a server-recorded provider choice disabled after remount", () => {
    const sendPromptToAgent = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      sendPromptToAgent,
      remoteQueueItems: [{
        id: "action-1",
        type: "action_needed",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "agent-1",
        agent_name: "Coder",
        summary: "Proceed?\n1. Yes",
        provider_choice_sent: "1",
      }],
    });

    render(<RemoteInboxView />);

    const choice = screen.getByRole("button", { name: "Send action response 1: Yes" });
    expect(choice).toBeDisabled();
    fireEvent.click(choice);
    expect(sendPromptToAgent).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Retry Inbox status" })).toBeVisible();
  });

  it("shows an uncertain state for a provider choice pending delivery confirmation", () => {
    const sendPromptToAgent = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      sendPromptToAgent,
      remoteQueueItems: [{
        id: "action-1",
        type: "action_needed",
        timestamp: Date.now(),
        read: true,
        agent_session_id: "agent-1",
        agent_name: "Coder",
        summary: "Proceed?\n1. Yes",
        provider_choice_pending: "1",
      }],
    });

    render(<RemoteInboxView />);

    expect(screen.getByRole("button", { name: "Send action response 1: Yes" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("Response delivery is uncertain");
    expect(screen.queryByRole("button", { name: "Clear item" })).not.toBeInTheDocument();
    expect(sendPromptToAgent).not.toHaveBeenCalled();
  });

  it("disables clear read when only durable notifications are read", () => {
    useRemoteStore.setState({
      remoteQueueItems: [{
        id: "notification:1",
        type: "agent_update",
        timestamp: Date.now(),
        read: true,
        inbox_notification_id: "1",
        agent_name: "Coder",
        summary: "Finished",
      }],
    });

    render(<RemoteInboxView />);

    expect(screen.getByRole("button", { name: "Clear read Inbox items" })).toBeDisabled();
  });

  it("keeps read action-needed prompts out of Clear read", () => {
    useRemoteStore.setState({
      remoteQueueItems: [{
        id: "read-action",
        type: "action_needed",
        timestamp: Date.now(),
        read: true,
        agent_name: "Coder",
        summary: "Choose an action",
      }],
    });

    render(<RemoteInboxView />);

    expect(screen.getByRole("button", { name: "Clear read Inbox items" })).toBeDisabled();
  });

  it("keeps a read pending provider choice out of Clear read", () => {
    useRemoteStore.setState({
      remoteQueueItems: [{
        id: "read-pending-completion",
        type: "automation_completed",
        timestamp: Date.now(),
        read: true,
        agent_name: "Coder",
        summary: "Choose an action",
        provider_choice_pending: "1",
      }],
    });

    render(<RemoteInboxView />);

    expect(screen.getByRole("button", { name: "Clear read Inbox items" })).toBeDisabled();
  });

  it("does not mark a pending manual approval read while navigating", () => {
    const runInboxAction = vi.fn().mockResolvedValue(undefined);
    const openAgent = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      runInboxAction,
      openAgent,
      remoteQueueItems: [{
        id: "approval-1",
        type: "approval_request",
        timestamp: Date.now(),
        read: false,
        agent_session_id: "agent-1",
        notification_title: "Deploy",
        summary: "Approve deployment?",
        notification_status: "awaiting_reply",
        approval_choices: ["Approve", "Reject"],
        inbox_notification_id: "notification-1",
      }],
    });

    render(<RemoteInboxView />);
    fireEvent.click(screen.getByText("Approve deployment?"));
    fireEvent.click(screen.getByRole("button", { name: "Open agent terminal" }));

    expect(openAgent).toHaveBeenCalledWith("agent-1");
    expect(runInboxAction).not.toHaveBeenCalledWith("mark_read", "approval-1");
  });

  it("shows header action failures and disables both actions while pending", async () => {
    const runInboxAction = vi.fn().mockRejectedValue(new Error("Remote request failed: 503"));
    useRemoteStore.setState({
      runInboxAction,
      remoteQueueItems: [{
        id: "unread-1",
        type: "agent_update",
        timestamp: Date.now(),
        read: false,
        agent_name: "Coder",
        summary: "Needs attention",
      }],
    });

    render(<RemoteInboxView />);

    const markAll = screen.getByRole("button", { name: "Mark all Inbox items read" });
    const clearRead = screen.getByRole("button", { name: "Clear read Inbox items" });
    fireEvent.click(markAll);

    expect(markAll).toBeDisabled();
    expect(clearRead).toBeDisabled();
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Remote request failed: 503"));
    expect(markAll).not.toBeDisabled();
  });

  it("marks a card read and dismisses legacy items", async () => {
    const runInboxAction = vi.fn().mockResolvedValue(undefined);
    useRemoteStore.setState({
      runInboxAction,
      remoteQueueItems: [{
        id: "legacy-1",
        type: "agent_completed",
        timestamp: Date.now(),
        read: false,
        agent_name: "Coder",
        summary: "Finished",
      }],
    });

    render(<RemoteInboxView />);
    fireEvent.click(screen.getByText("Finished"));
    fireEvent.click(screen.getByRole("button", { name: "Clear item" }));

    expect(runInboxAction).toHaveBeenCalledWith("mark_read", "legacy-1");
    expect(runInboxAction).toHaveBeenCalledWith("dismiss", "legacy-1", undefined);
  });
});
