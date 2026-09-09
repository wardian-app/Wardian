import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConfig } from "../../types";
import { useQueueStore } from "../../store/useQueueStore";
import { GardenAgentInterior, type GardenAgentInteriorProps } from "./GardenAgentInterior";
import { useGardenAgentContents, type GardenMemoryRecord } from "./useGardenAgentContents";

vi.mock("./useGardenAgentContents", () => ({ useGardenAgentContents: vi.fn() }));
const agent: AgentConfig = { session_id: "a", session_name: "Agent A", agent_class: "Builder", folder: "/workspace", provider: "codex", is_off: false };
const memory: GardenMemoryRecord = {
  memory_id: "m1", revision_id: "r1", revision: 1, agent_id: "a", workspace: null, kind: "stable",
  text: "Keep evidence", evidence_excerpt: "Sources matter", evidence_hash: "hash", status: "active",
  supersedes_revision_id: null, replaced_by_revision_id: null, created_at: "now", updated_at: "now",
  last_verified_at: "now", idempotency_key: null, sources: [],
};
function props(): GardenAgentInteriorProps {
  return {
    agent, status: "idle", agents: [agent, { ...agent, session_id: "b", session_name: "Agent B" }],
    teams: [{ id: "team", name: "Team", agentIds: ["a", "b"] }],
    automations: [{ id: "routine", label: "Review", agentIds: ["a"], nodeCount: 2, runStatus: "running" }],
    crown: [{ entryRef: "skills/check", label: "Check", monogram: "C", hue: 20, provenance: "class", copied: true }],
    onSelect: vi.fn(), onEnter: vi.fn(), onOpenAgent: vi.fn(),
  };
}
beforeEach(() => {
  useQueueStore.setState({ items: [], loadItems: vi.fn().mockResolvedValue(undefined), inboxNotificationsTruncated: false });
  vi.mocked(useGardenAgentContents).mockReturnValue({
    memories: { data: [memory], error: null, stale: false, loading: false },
    conversations: { data: [], error: null, stale: false, loading: false }, refresh: vi.fn(),
  });
});

describe("GardenAgentInterior", () => {
  it("keeps memories with the same compact caption distinguishable by accessible name and tooltip", () => {
    const prefix = "Preserve the same agent geography when inspecting ";
    const records = [
      { ...memory, memory_id: "one", text: `${prefix}successful executions.` },
      { ...memory, memory_id: "two", text: `${prefix}failed executions.` },
    ];
    vi.mocked(useGardenAgentContents).mockReturnValue({
      memories: { data: records, error: null, stale: false, loading: false },
      conversations: { data: [], error: null, stale: false, loading: false }, refresh: vi.fn(),
    });
    render(<GardenAgentInterior {...props()} />);
    for (const record of records) {
      const button = screen.getByRole("button", { name: `${record.text} Revision 1` });
      expect(button).toHaveAttribute("title", `${record.text} · Revision 1`);
      expect(button.querySelector("strong")?.textContent).not.toContain("executions");
    }
  });
  it("uses Wardian feature labels while retaining the five geographic trays", async () => {
    render(<GardenAgentInterior {...props()} />);
    for (const name of ["Identity", "Skills", "Memory"]) {
      expect(within(screen.getByRole("region", { name })).getByRole("heading", { level: 3 })).toHaveAccessibleName(name);
    }
    expect(screen.getAllByRole("region")).toHaveLength(5);
    expect(screen.getByRole("region", { name: "Skills" })).toHaveClass("garden-agent-interior-capabilities");
    expect(screen.getByRole("region", { name: "Automations, Conversations, Inbox" })).toHaveClass("garden-agent-interior-active-work");
    expect(screen.getByRole("region", { name: "Workspace, Teams, Agents" })).toHaveClass("garden-agent-interior-ports");
    for (const name of ["Automations", "Workspace", "Teams", "Agents"]) expect(screen.getByRole("group", { name })).toBeInTheDocument();
    for (const name of ["Conversations", "Inbox"]) expect(screen.getByLabelText(name)).toBeInTheDocument();
    expect(screen.getByText("Tools", { selector: "summary" })).toBeInTheDocument();
    for (const name of ["Capabilities", "Active work", "Ports"]) expect(screen.queryByText(name)).not.toBeInTheDocument();
    expect(screen.getByText("Class-inherited · Copied; does not sync")).toBeInTheDocument();
    expect(screen.getByText("Agent-wide")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("No attributable items in the loaded Inbox.")).toBeInTheDocument());
  });

  it("selects without entering and exposes selected canonical memory identity", () => {
    const callbacks = props();
    render(<GardenAgentInterior {...callbacks} selectedKey="memory:m1" />);
    const button = screen.getByRole("button", { name: /Keep evidence/ });
    expect(button).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(button);
    expect(callbacks.onSelect).toHaveBeenCalledWith({ kind: "memory", id: "m1" });
    expect(callbacks.onEnter).not.toHaveBeenCalled();
    fireEvent.doubleClick(button);
    expect(callbacks.onEnter).toHaveBeenCalledWith({ kind: "memory", id: "m1" });
    vi.mocked(callbacks.onEnter).mockClear();
    fireEvent.keyDown(button, { key: "Enter" });
    expect(callbacks.onEnter).toHaveBeenCalledTimes(1);
  });

  it("routes identity, skills, workspace and related agents with canonical refs", () => {
    const callbacks = props();
    render(<GardenAgentInterior {...callbacks} />);
    for (const [name, ref] of [
      [/Agent A/, { kind: "identity", id: "a" }],
      [/Check/, { kind: "skill", id: "skills/check" }],
      [/\/workspace Workspace/, { kind: "workspace", id: "/workspace" }],
      [/Agent B/, { kind: "agent", id: "b" }],
    ] as const) {
      fireEvent.doubleClick(screen.getByRole("button", { name }));
      expect(callbacks.onEnter).toHaveBeenLastCalledWith(ref);
    }
    fireEvent.click(screen.getByRole("button", { name: "Open agent session" }));
    expect(callbacks.onOpenAgent).toHaveBeenCalledWith("a");
  });

  it("preserves regions and old content when a refresh fails", () => {
    vi.mocked(useGardenAgentContents).mockReturnValue({
      memories: { data: [memory], error: "Offline", stale: true, loading: false },
      conversations: { data: null, error: "Permission denied", stale: false, loading: false }, refresh: vi.fn(),
    });
    render(<GardenAgentInterior {...props()} />);
    expect(screen.getAllByRole("region")).toHaveLength(5);
    expect(screen.getByText("Showing the last loaded snapshot.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Keep evidence/ })).toBeInTheDocument();
    expect(screen.getByText("Conversations unavailable: Permission denied")).toBeInTheDocument();
  });

  it("shows only stable-session-attributed Inbox items and reacts to updates", async () => {
    useQueueStore.setState({ items: [
      { id: "own", type: "approval_request", agent_session_id: "a", summary: "Review this change", notification_status: "awaiting_reply", timestamp: 1, read: false },
      { id: "other", type: "agent_completed", agent_session_id: "b", agent_name: "Agent A", summary: "Other agent output", timestamp: 2, read: false },
      { id: "unbound", type: "automation_completed", automation_id: "routine", summary: "Unattributed run", timestamp: 3, read: false },
      { id: "dismissed", type: "agent_completed", agent_session_id: "a", summary: "Dismissed output", dismissed: true, timestamp: 4, read: true },
    ] });
    const { rerender } = render(<GardenAgentInterior {...props()} />);
    await waitFor(() => expect(useQueueStore.getState().loadItems).toHaveBeenCalledOnce());
    expect(screen.getByText("Review this change")).toBeInTheDocument();
    expect(screen.getByText("Awaiting approval · Unread")).toBeInTheDocument();
    expect(screen.queryByText("Other agent output")).not.toBeInTheDocument();
    expect(screen.queryByText("Unattributed run")).not.toBeInTheDocument();
    expect(screen.queryByText("Dismissed output")).not.toBeInTheDocument();
    act(() => useQueueStore.setState({ items: [{ id: "new", type: "agent_completed", agent_session_id: "a", summary: "Completed output", timestamp: 5, read: true }] }));
    expect(screen.getByText("Completed output")).toBeInTheDocument();
    expect(screen.getByText("Completed · Read")).toBeInTheDocument();
    rerender(<GardenAgentInterior {...props()} agent={{ ...agent, session_id: "b" }} />);
    expect(screen.queryByText("Completed output")).not.toBeInTheDocument();
  });

  it("offers canonical Inbox pagination when attributable items may be in older pages", async () => {
    const loadMore = vi.fn().mockResolvedValue(undefined);
    useQueueStore.setState({ inboxNotificationsTruncated: true, loadMoreInboxNotifications: loadMore });
    render(<GardenAgentInterior {...props()} />);
    fireEvent.click(screen.getByRole("button", { name: "Load older Inbox items" }));
    expect(loadMore).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.queryByText("Loading Inbox…")).not.toBeInTheDocument());
  });

  it("shows canonical configured tools and permissions ahead of legacy fields", () => {
    render(<GardenAgentInterior {...props()} agent={{ ...agent, provider: "claude", permission_mode: "bypassPermissions", allowed_tools: ["Legacy"], provider_config: {
      type: "claude", permission_mode: "plan", tools: ["Read", "Edit"], allowed_tools: ["Read"], disallowed_tools: ["Bash"], strict_mcp_config: false,
    } }} />);
    const identity = within(screen.getByRole("region", { name: "Identity" }));
    const skills = within(screen.getByRole("region", { name: "Skills" }));
    expect(identity.getByText("plan")).toBeInTheDocument();
    expect(identity.getByText("No")).toBeInTheDocument();
    expect(skills.getByText("Read, Edit")).toBeInTheDocument();
    expect(skills.getByText("Bash")).toBeInTheDocument();
    expect(screen.queryByText("Legacy")).not.toBeInTheDocument();
    expect(screen.queryByText("bypassPermissions")).not.toBeInTheDocument();
  });

  it("normalizes legacy Codex permissions and preserves explicit disabled search", () => {
    render(<GardenAgentInterior {...props()} agent={{ ...agent, codex_sandbox_mode: "read-only", codex_approval_policy: "never", codex_search: false }} />);
    expect(screen.getByText("read-only")).toBeInTheDocument();
    expect(screen.getByText("never")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Skills" })).getByText("No")).toBeInTheDocument();
  });
});
