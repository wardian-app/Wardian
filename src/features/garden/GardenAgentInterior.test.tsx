import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConfig } from "../../types";
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
    agent, status: "idle",
    automations: [{ id: "routine", label: "Review", agentIds: ["a"], nodeCount: 2, runStatus: "running" }],
    crown: [{ entryRef: "skills/check", label: "Check", monogram: "C", hue: 20, provenance: "class", copied: true }],
    onSelect: vi.fn(), onEnter: vi.fn(), onOpenAgent: vi.fn(),
  };
}
beforeEach(() => {
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
  it("uses five direct Wardian organelles without secondary feature drawers", () => {
    render(<GardenAgentInterior {...props()} />);
    for (const name of ["Identity", "Skills", "Memory", "Automations", "Conversations"]) {
      expect(within(screen.getByRole("region", { name })).getByRole("heading", { level: 3 })).toHaveAccessibleName(name);
    }
    expect(screen.getAllByRole("region")).toHaveLength(5);
    expect(screen.getByRole("region", { name: "Skills" })).toHaveClass("garden-agent-interior-capabilities");
    expect(screen.getByRole("region", { name: "Automations" })).toHaveClass("garden-agent-interior-automations");
    expect(screen.getByRole("region", { name: "Conversations" })).toHaveClass("garden-agent-interior-conversations");
    for (const name of ["Tools", "Inbox", "Workspace", "Teams", "Agents", "Capabilities", "Active work", "Ports"]) {
      expect(screen.queryByText(name)).not.toBeInTheDocument();
    }
    expect(screen.getByText("Class-inherited · Copied; does not sync")).toBeInTheDocument();
    expect(screen.getByText("Agent-wide")).toBeInTheDocument();
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

  it("routes identity and skills with canonical refs", () => {
    const callbacks = props();
    render(<GardenAgentInterior {...callbacks} />);
    for (const [name, ref] of [
      [/Agent A/, { kind: "identity", id: "a" }],
      [/Check/, { kind: "skill", id: "skills/check" }],
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

  it("shows canonical permissions without adding a Tools drawer", () => {
    render(<GardenAgentInterior {...props()} agent={{ ...agent, provider: "claude", permission_mode: "bypassPermissions", allowed_tools: ["Legacy"], provider_config: {
      type: "claude", permission_mode: "plan", tools: ["Read", "Edit"], allowed_tools: ["Read"], disallowed_tools: ["Bash"], strict_mcp_config: false,
    } }} />);
    const identity = within(screen.getByRole("region", { name: "Identity" }));
    expect(identity.getByText("plan")).toBeInTheDocument();
    expect(identity.getByText("No")).toBeInTheDocument();
    expect(screen.queryByText("Read, Edit")).not.toBeInTheDocument();
    expect(screen.queryByText("Bash")).not.toBeInTheDocument();
    expect(screen.queryByText("Legacy")).not.toBeInTheDocument();
    expect(screen.queryByText("bypassPermissions")).not.toBeInTheDocument();
  });

  it("normalizes legacy Codex permissions without projecting tool configuration", () => {
    render(<GardenAgentInterior {...props()} agent={{ ...agent, codex_sandbox_mode: "read-only", codex_approval_policy: "never", codex_search: false }} />);
    expect(screen.getByText("read-only")).toBeInTheDocument();
    expect(screen.getByText("never")).toBeInTheDocument();
    expect(screen.queryByText("Web search")).not.toBeInTheDocument();
  });
});
